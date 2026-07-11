// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK production scanout-slot mappings.
 *
 * Exposes the two framebuffer-reserved hidden RGB565 slots through one
 * write-combined mapping interface. The FPGA vblank latch owns route changes;
 * this module neither allocates scanout memory nor creates cacheable aliases.
 */

#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <generated/utsrelease.h>

#define DEVICE_NAME "mister-magik-scanout-slots"
#define SCANOUT_SLOTS_VERSION 1

#define FB_PHYS_BASE 0x22000000UL
#define RGB565_WIDTH 960UL
#define RGB565_HEIGHT 540UL
#define RGB565_STRIDE_BYTES 1920UL
#define RGB565_FRAME_BYTES (RGB565_STRIDE_BYTES * RGB565_HEIGHT)
#define RGB565_MAP_BYTES PAGE_ALIGN(RGB565_FRAME_BYTES)
#define HIDDEN_SLOT_BYTES (1920UL * 1080UL * 4UL)

#define REGION_HIDDEN_SLOT1 0UL
#define REGION_HIDDEN_SLOT2 1UL
#define REGION_COUNT 2UL
#define REGION_OFFSET_PAGES 256UL

struct probe_region {
	const char *name;
	unsigned long phys;
	unsigned long len;
};

static atomic_t open_count = ATOMIC_INIT(0);
static atomic_t mmap_count = ATOMIC_INIT(0);
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
	if (!region->len)
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
			  "scanout_slots_header_tsv\tname=%s\tversion=%u\tuts_release=%s\topen_count=%d\tmmap_count=%d\tpage_size=%lu\tregion_offset_pages=%lu\tregion_offset_bytes=%lu\tcache_mode=writecombine\n",
			  DEVICE_NAME, SCANOUT_SLOTS_VERSION, UTS_RELEASE,
			  atomic_read(&open_count), atomic_read(&mmap_count),
			  PAGE_SIZE, REGION_OFFSET_PAGES,
			  REGION_OFFSET_PAGES * PAGE_SIZE);
	used += scnprintf(tmp + used, 4096 - used,
			  "scanout_slots_expected_tsv\twidth=%lu\theight=%lu\tstride_bytes=%lu\tframe_bytes=%lu\thidden_slot_bytes=%lu\tfb_phys_base=0x%08lx\n",
			  RGB565_WIDTH, RGB565_HEIGHT, RGB565_STRIDE_BYTES,
			  RGB565_FRAME_BYTES, HIDDEN_SLOT_BYTES, FB_PHYS_BASE);

	for (i = 0; i < REGION_COUNT; i++) {
		used += scnprintf(tmp + used, 4096 - used,
				  "scanout_slots_region_tsv\tindex=%d\tname=%s\tavailable=1\tphys=0x%08lx\tlen=%lu\n",
				  i, regions[i].name, regions[i].phys, regions[i].len);
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

	regions[REGION_HIDDEN_SLOT1] = (struct probe_region) {
		.name = "hidden-slot-1",
		.phys = FB_PHYS_BASE + HIDDEN_SLOT_BYTES,
		.len = RGB565_MAP_BYTES,
	};
	regions[REGION_HIDDEN_SLOT2] = (struct probe_region) {
		.name = "hidden-slot-2",
		.phys = FB_PHYS_BASE + (HIDDEN_SLOT_BYTES * 2UL),
		.len = RGB565_MAP_BYTES,
	};

	ret = misc_register(&probe_miscdev);
	if (ret)
		return ret;

	pr_info("mister_magik_scanout_slots: loaded /dev/%s version=%u\n",
		DEVICE_NAME, SCANOUT_SLOTS_VERSION);
	return 0;
}

static void __exit probe_exit(void)
{
	misc_deregister(&probe_miscdev);
	pr_info("mister_magik_scanout_slots: unloaded\n");
}

module_init(probe_init);
module_exit(probe_exit);

MODULE_DESCRIPTION("MiSTer MagiK write-combined scanout slot mappings");
MODULE_AUTHOR("MiSTer MagiK");
MODULE_LICENSE("GPL v2");
