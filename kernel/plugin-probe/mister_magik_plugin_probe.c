// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK stock-kernel plugin feasibility probe.
 *
 * Diagnostics only: exposes mmap ranges through a misc device and reports
 * metadata. It does not replace MiSTer_fb, change scanout, or install itself
 * persistently.
 */

#include <linux/dma-mapping.h>
#include <linux/fs.h>
#include <linux/io.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>

#define DEVICE_NAME "mister-magik-plugin-probe"
#define PROBE_VERSION 1

#define FB_PHYS_BASE 0x22000000UL
#define FB_CONTROL_BYTES 4096UL
#define RGB565_WIDTH 960UL
#define RGB565_HEIGHT 540UL
#define RGB565_STRIDE_BYTES 1920UL
#define RGB565_FRAME_BYTES (RGB565_STRIDE_BYTES * RGB565_HEIGHT)
#define RGB565_MAP_BYTES PAGE_ALIGN(RGB565_FRAME_BYTES)
#define HIDDEN_SLOT_BYTES (1920UL * 1080UL * 4UL)

#define REGION_ADJACENT_FB 0UL
#define REGION_HIDDEN_SLOT1 1UL
#define REGION_HIDDEN_SLOT2 2UL
#define REGION_PLUGIN_DMA 3UL
#define REGION_COUNT 4UL
#define REGION_OFFSET_PAGES 256UL

struct probe_region {
	const char *name;
	unsigned long phys;
	unsigned long len;
	bool available;
	bool dma_owned;
};

static atomic_t open_count = ATOMIC_INIT(0);
static atomic_t mmap_count = ATOMIC_INIT(0);
static void *dma_virt;
static dma_addr_t dma_handle;
static struct probe_region regions[REGION_COUNT];

static int probe_open(struct inode *inode, struct file *file)
{
	atomic_inc(&open_count);
	return 0;
}

static int probe_release(struct inode *inode, struct file *file)
{
	atomic_dec(&open_count);
	return 0;
}

static void probe_vma_close(struct vm_area_struct *vma)
{
	atomic_dec(&mmap_count);
}

static const struct vm_operations_struct probe_vm_ops = {
	.close = probe_vma_close,
};

static int probe_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long size = vma->vm_end - vma->vm_start;
	unsigned long region_index = vma->vm_pgoff / REGION_OFFSET_PAGES;
	unsigned long region_page_offset = vma->vm_pgoff % REGION_OFFSET_PAGES;
	unsigned long offset = region_page_offset << PAGE_SHIFT;
	struct probe_region *region;
	unsigned long pfn;

	if (region_index >= REGION_COUNT)
		return -EINVAL;

	region = &regions[region_index];
	if (!region->available || !region->len)
		return -ENODEV;

	if (offset >= region->len)
		return -EINVAL;
	if (size > region->len - offset)
		return -EINVAL;

	pfn = (region->phys >> PAGE_SHIFT) + region_page_offset;
	vma->vm_page_prot = pgprot_writecombine(vma->vm_page_prot);
	vma->vm_flags |= VM_IO | VM_DONTEXPAND | VM_DONTDUMP;

	if (remap_pfn_range(vma, vma->vm_start, pfn, size, vma->vm_page_prot))
		return -EAGAIN;

	vma->vm_ops = &probe_vm_ops;
	atomic_inc(&mmap_count);
	return 0;
}

static ssize_t probe_read(struct file *file, char __user *buf, size_t len,
			  loff_t *ppos)
{
	char *tmp;
	size_t used = 0;
	int i;
	ssize_t ret;

	tmp = kzalloc(2048, GFP_KERNEL);
	if (!tmp)
		return -ENOMEM;

	used += scnprintf(tmp + used, 2048 - used,
			  "plugin_probe_header_tsv\tname=%s\tversion=%u\tuts_release=%s\topen_count=%d\tmmap_count=%d\tpage_size=%lu\tregion_offset_pages=%lu\n",
			  DEVICE_NAME, PROBE_VERSION, UTS_RELEASE,
			  atomic_read(&open_count), atomic_read(&mmap_count),
			  PAGE_SIZE, REGION_OFFSET_PAGES);
	used += scnprintf(tmp + used, 2048 - used,
			  "plugin_probe_expected_tsv\twidth=%lu\theight=%lu\tstride_bytes=%lu\tframe_bytes=%lu\thidden_slot_bytes=%lu\tfb_phys_base=0x%08lx\tfb_active_phys=0x%08lx\n",
			  RGB565_WIDTH, RGB565_HEIGHT, RGB565_STRIDE_BYTES,
			  RGB565_FRAME_BYTES, HIDDEN_SLOT_BYTES, FB_PHYS_BASE,
			  FB_PHYS_BASE + FB_CONTROL_BYTES);

	for (i = 0; i < REGION_COUNT; i++) {
		used += scnprintf(tmp + used, 2048 - used,
				  "plugin_probe_region_tsv\tindex=%d\tname=%s\tavailable=%u\tphys=0x%08lx\tlen=%lu\tdma_owned=%u\n",
				  i, regions[i].name, regions[i].available ? 1 : 0,
				  regions[i].phys, regions[i].len,
				  regions[i].dma_owned ? 1 : 0);
	}

	ret = simple_read_from_buffer(buf, len, ppos, tmp, used);
	kfree(tmp);
	return ret;
}

static const struct file_operations probe_fops = {
	.owner = THIS_MODULE,
	.open = probe_open,
	.release = probe_release,
	.read = probe_read,
	.mmap = probe_mmap,
	.llseek = no_llseek,
};

static struct miscdevice probe_miscdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = DEVICE_NAME,
	.fops = &probe_fops,
	.mode = 0600,
};

static int __init probe_init(void)
{
	int ret;

	regions[REGION_ADJACENT_FB] = (struct probe_region) {
		.name = "adjacent-fb-resource",
		.phys = FB_PHYS_BASE + FB_CONTROL_BYTES + RGB565_FRAME_BYTES,
		.len = RGB565_MAP_BYTES,
		.available = true,
		.dma_owned = false,
	};
	regions[REGION_HIDDEN_SLOT1] = (struct probe_region) {
		.name = "hidden-slot1",
		.phys = FB_PHYS_BASE + HIDDEN_SLOT_BYTES,
		.len = RGB565_MAP_BYTES,
		.available = true,
		.dma_owned = false,
	};
	regions[REGION_HIDDEN_SLOT2] = (struct probe_region) {
		.name = "hidden-slot2",
		.phys = FB_PHYS_BASE + (HIDDEN_SLOT_BYTES * 2UL),
		.len = RGB565_MAP_BYTES,
		.available = true,
		.dma_owned = false,
	};
	regions[REGION_PLUGIN_DMA] = (struct probe_region) {
		.name = "plugin-owned-dma",
		.phys = 0,
		.len = RGB565_MAP_BYTES,
		.available = false,
		.dma_owned = true,
	};

	ret = misc_register(&probe_miscdev);
	if (ret)
		return ret;

	ret = dma_set_mask_and_coherent(probe_miscdev.this_device, DMA_BIT_MASK(32));
	if (ret) {
		pr_warn("mister_magik_plugin_probe: 32-bit DMA mask rejected; plugin-owned-dma unavailable\n");
	} else {
		dma_virt = dma_alloc_wc(probe_miscdev.this_device, RGB565_MAP_BYTES,
					&dma_handle, GFP_KERNEL);
		if (dma_virt) {
			regions[REGION_PLUGIN_DMA].phys = (unsigned long)dma_handle;
			regions[REGION_PLUGIN_DMA].available = true;
		} else {
			pr_warn("mister_magik_plugin_probe: dma_alloc_wc failed; plugin-owned-dma unavailable\n");
		}
	}

	pr_info("mister_magik_plugin_probe: loaded /dev/%s version=%u\n",
		DEVICE_NAME, PROBE_VERSION);
	return 0;
}

static void __exit probe_exit(void)
{
	if (dma_virt)
		dma_free_wc(probe_miscdev.this_device, RGB565_MAP_BYTES,
			    dma_virt, dma_handle);
	misc_deregister(&probe_miscdev);
	pr_info("mister_magik_plugin_probe: unloaded\n");
}

module_init(probe_init);
module_exit(probe_exit);

MODULE_DESCRIPTION("MiSTer MagiK stock-kernel plugin feasibility probe");
MODULE_AUTHOR("MiSTer MagiK");
MODULE_LICENSE("GPL v2");
