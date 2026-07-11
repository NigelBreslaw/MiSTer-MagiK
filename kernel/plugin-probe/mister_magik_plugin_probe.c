// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK scanout plugin for the stock MiSTer kernel.
 *
 * /dev/mister-magik-scanout is the production interface. The historical
 * /dev/mister-magik-plugin-probe diagnostics interface remains for one
 * compatibility release; it does not own or alias the scanout allocations.
 */

#include <linux/dma-mapping.h>
#include <linux/fs.h>
#include <linux/io.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>
#include "mister_magik_scanout_uapi.h"

#define DEVICE_NAME "mister-magik-plugin-probe"
#define SCANOUT_DEVICE_NAME "mister-magik-scanout"
#define PROBE_VERSION 4

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
static struct probe_region regions[REGION_COUNT];
static struct platform_device *scanout_pdev;
static bool probe_registered;
static void *scanout_virt[MISTER_MAGIK_SCANOUT_SLOT_COUNT];
static dma_addr_t scanout_dma[MISTER_MAGIK_SCANOUT_SLOT_COUNT];
static const size_t scanout_slot_bytes = 1280UL * 720UL * 2UL;
static const size_t scanout_map_bytes = PAGE_ALIGN(1280UL * 720UL * 2UL);

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

	tmp = kzalloc(4096, GFP_KERNEL);
	if (!tmp)
		return -ENOMEM;

	used += scnprintf(tmp + used, 4096 - used,
			  "plugin_probe_header_tsv\tname=%s\tversion=%u\tuts_release=%s\topen_count=%d\tmmap_count=%d\tpage_size=%lu\tregion_offset_pages=%lu\tregion_offset_bytes=%lu\tcache_mode=writecombine\n",
			  DEVICE_NAME, PROBE_VERSION, UTS_RELEASE,
			  atomic_read(&open_count), atomic_read(&mmap_count),
			  PAGE_SIZE, REGION_OFFSET_PAGES,
			  REGION_OFFSET_PAGES * PAGE_SIZE);
	used += scnprintf(tmp + used, 4096 - used,
			  "plugin_probe_expected_tsv\twidth=%lu\theight=%lu\tstride_bytes=%lu\tframe_bytes=%lu\thidden_slot_bytes=%lu\tfb_phys_base=0x%08lx\tfb_active_phys=0x%08lx\n",
			  RGB565_WIDTH, RGB565_HEIGHT, RGB565_STRIDE_BYTES,
			  RGB565_FRAME_BYTES, HIDDEN_SLOT_BYTES, FB_PHYS_BASE,
			  FB_PHYS_BASE + FB_CONTROL_BYTES);

	for (i = 0; i < REGION_COUNT; i++) {
		used += scnprintf(tmp + used, 4096 - used,
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

static long scanout_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct mister_magik_scanout_caps caps = { };
	struct mister_magik_scanout_sync sync;
	__u32 slot;
	unsigned int i;

	switch (cmd) {
	case MISTER_MAGIK_SCANOUT_GET_CAPS:
		caps.abi_version = MISTER_MAGIK_SCANOUT_ABI_VERSION;
		caps.slot_count = MISTER_MAGIK_SCANOUT_SLOT_COUNT;
		caps.slot_bytes = scanout_slot_bytes;
		caps.mmap_stride = scanout_map_bytes;
		for (i = 0; i < MISTER_MAGIK_SCANOUT_SLOT_COUNT; i++)
			caps.dma_addr[i] = (__u32)scanout_dma[i];
		return copy_to_user((void __user *)arg, &caps, sizeof(caps)) ? -EFAULT : 0;
	case MISTER_MAGIK_SCANOUT_ACQUIRE_CPU:
		if (copy_from_user(&slot, (void __user *)arg, sizeof(slot)))
			return -EFAULT;
		if (slot >= MISTER_MAGIK_SCANOUT_SLOT_COUNT)
			return -EINVAL;
		dma_sync_single_for_cpu(&scanout_pdev->dev, scanout_dma[slot],
					scanout_slot_bytes, DMA_TO_DEVICE);
		return 0;
	case MISTER_MAGIK_SCANOUT_SYNC_DEVICE:
		if (copy_from_user(&sync, (void __user *)arg, sizeof(sync)))
			return -EFAULT;
		if (sync.slot >= MISTER_MAGIK_SCANOUT_SLOT_COUNT ||
		    sync.range_count > MISTER_MAGIK_SCANOUT_MAX_RANGES)
			return -EINVAL;
		for (i = 0; i < sync.range_count; i++) {
			__u32 offset = sync.ranges[i].offset;
			__u32 length = sync.ranges[i].length;

			if (!length || offset >= scanout_slot_bytes ||
			    length > scanout_slot_bytes - offset)
				return -EINVAL;
			dma_sync_single_range_for_device(&scanout_pdev->dev,
				scanout_dma[sync.slot], offset, length, DMA_TO_DEVICE);
		}
		return 0;
	default:
		return -ENOTTY;
	}
}

static int scanout_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long size = vma->vm_end - vma->vm_start;
	unsigned long byte_offset = vma->vm_pgoff << PAGE_SHIFT;
	unsigned int slot = byte_offset / scanout_map_bytes;

	if (slot >= MISTER_MAGIK_SCANOUT_SLOT_COUNT ||
	    byte_offset % scanout_map_bytes || size > scanout_slot_bytes)
		return -EINVAL;
	vma->vm_flags |= VM_DONTEXPAND | VM_DONTDUMP;
	return remap_pfn_range(vma, vma->vm_start,
			virt_to_pfn(scanout_virt[slot]), size, vma->vm_page_prot);
}

static const struct file_operations scanout_fops = {
	.owner = THIS_MODULE,
	.unlocked_ioctl = scanout_ioctl,
	.mmap = scanout_mmap,
	.llseek = no_llseek,
};

static struct miscdevice scanout_miscdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = SCANOUT_DEVICE_NAME,
	.fops = &scanout_fops,
	.mode = 0600,
};

static void free_scanout_slots(void)
{
	int i;

	if (!scanout_pdev)
		return;
	for (i = 0; i < MISTER_MAGIK_SCANOUT_SLOT_COUNT; i++) {
		if (scanout_virt[i])
			dma_free_noncoherent(&scanout_pdev->dev, scanout_map_bytes,
				scanout_virt[i], scanout_dma[i], DMA_TO_DEVICE);
		scanout_virt[i] = NULL;
	}
}

static int init_scanout_slots(void)
{
	int i, ret;

	scanout_pdev = platform_device_register_simple(SCANOUT_DEVICE_NAME, -1, NULL, 0);
	if (IS_ERR(scanout_pdev)) {
		ret = PTR_ERR(scanout_pdev);
		scanout_pdev = NULL;
		return ret;
	}
	scanout_pdev->dev.coherent_dma_mask = DMA_BIT_MASK(32);
	scanout_pdev->dev.dma_mask = &scanout_pdev->dev.coherent_dma_mask;
	for (i = 0; i < MISTER_MAGIK_SCANOUT_SLOT_COUNT; i++) {
		scanout_virt[i] = dma_alloc_noncoherent(&scanout_pdev->dev,
			scanout_map_bytes, &scanout_dma[i], DMA_TO_DEVICE, GFP_KERNEL);
		if (!scanout_virt[i]) {
			ret = -ENOMEM;
			goto fail;
		}
	}
	ret = misc_register(&scanout_miscdev);
	if (ret)
		goto fail;
	return 0;
fail:
	free_scanout_slots();
	platform_device_unregister(scanout_pdev);
	scanout_pdev = NULL;
	return ret;
}

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
		.name = "hidden-slot-1",
		.phys = FB_PHYS_BASE + HIDDEN_SLOT_BYTES,
		.len = RGB565_MAP_BYTES,
		.available = true,
		.dma_owned = false,
	};
	regions[REGION_HIDDEN_SLOT2] = (struct probe_region) {
		.name = "hidden-slot-2",
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

	ret = init_scanout_slots();
	if (ret) {
		pr_err("mister_magik_scanout: production scanout unavailable error=%d\n", ret);
		return ret;
	}

	ret = misc_register(&probe_miscdev);
	if (ret)
		pr_warn("mister_magik_scanout: compatibility probe unavailable error=%d\n", ret);
	else
		probe_registered = true;

	pr_info("mister_magik_scanout: loaded /dev/%s; compatibility_probe=%u version=%u\n",
		SCANOUT_DEVICE_NAME, probe_registered ? 1 : 0, PROBE_VERSION);
	return 0;
}

static void __exit probe_exit(void)
{
	if (scanout_pdev) {
		misc_deregister(&scanout_miscdev);
		free_scanout_slots();
		platform_device_unregister(scanout_pdev);
		scanout_pdev = NULL;
	}
	if (probe_registered) {
		misc_deregister(&probe_miscdev);
		probe_registered = false;
	}
	pr_info("mister_magik_scanout: unloaded\n");
}

module_init(probe_init);
module_exit(probe_exit);

MODULE_DESCRIPTION("MiSTer MagiK stock-kernel scanout plugin");
MODULE_AUTHOR("MiSTer MagiK");
MODULE_LICENSE("GPL v2");
