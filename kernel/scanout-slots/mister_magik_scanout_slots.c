// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK production scanout-slot mappings.
 *
 * Exposes exactly two pinned-platform hidden RGB565 scanout slots through one
 * bounded write-combined mapping interface. The FPGA vblank latch owns route
 * changes; this module neither allocates memory nor controls presentation.
 */

#include <linux/build_bug.h>
#include <linux/fb.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/ioport.h>
#include <linux/of.h>
#include <linux/of_address.h>
#include <linux/overflow.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>

#include "mister_magik_scanout_slots_uapi.h"
#include "mister_magik_scanout_platform.h"
#include "mister_magik_scanout_policy.h"

#define DEVICE_NAME "mister-magik-scanout-slots"
#define RGB565_WIDTH MISTER_MAGIK_PLATFORM_WIDTH
#define RGB565_HEIGHT MISTER_MAGIK_PLATFORM_HEIGHT
#define RGB565_STRIDE_BYTES MISTER_MAGIK_PLATFORM_STRIDE_BYTES
#define RGB565_FRAME_BYTES MISTER_MAGIK_PLATFORM_FRAME_BYTES
#define RGB565_MAP_BYTES MISTER_MAGIK_PLATFORM_MAP_BYTES
#define REGION_OFFSET_BYTES MISTER_MAGIK_PLATFORM_SLOT1_SELECTOR_BYTES
#define REGION_OFFSET_PAGES (REGION_OFFSET_BYTES / PAGE_SIZE)

struct scanout_slot {
	unsigned long phys;
	unsigned long mmap_pgoff;
};

static const struct scanout_slot scanout_slots[] = {
	{
		.phys = MISTER_MAGIK_PLATFORM_SLOT0_PHYS,
		.mmap_pgoff = 0,
	},
	{
		.phys = MISTER_MAGIK_PLATFORM_SLOT1_PHYS,
		.mmap_pgoff = REGION_OFFSET_PAGES,
	},
};

static void *scanout_slot_resources[MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT];
static struct resource framebuffer_resource;

static const struct mister_magik_scanout_slots_layout scanout_slots_layout = {
	.abi_version = MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION,
	.slot_count = MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT,
	.width = RGB565_WIDTH,
	.height = RGB565_HEIGHT,
	.stride_bytes = RGB565_STRIDE_BYTES,
	.frame_bytes = RGB565_FRAME_BYTES,
	.map_bytes = RGB565_MAP_BYTES,
	.flags = MISTER_MAGIK_SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE,
	.slots = {
		{
			.physical_address = MISTER_MAGIK_PLATFORM_SLOT0_PHYS,
			.mmap_offset_bytes = 0,
		},
		{
			.physical_address = MISTER_MAGIK_PLATFORM_SLOT1_PHYS,
			.mmap_offset_bytes = REGION_OFFSET_BYTES,
		},
	},
};

static int scanout_slots_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long size = vma->vm_end - vma->vm_start;
	const struct scanout_slot *slot = NULL;
	unsigned int i;

	if (size != RGB565_MAP_BYTES || !(vma->vm_flags & VM_SHARED) ||
	    !(vma->vm_flags & VM_READ) || !(vma->vm_flags & VM_WRITE) ||
	    (vma->vm_flags & VM_EXEC))
		return -EINVAL;

	for (i = 0; i < ARRAY_SIZE(scanout_slots); i++) {
		if (vma->vm_pgoff == scanout_slots[i].mmap_pgoff) {
			slot = &scanout_slots[i];
			break;
		}
	}
	if (!slot)
		return -EINVAL;

	vma->vm_page_prot = pgprot_writecombine(vma->vm_page_prot);
	vma->vm_flags &= ~(VM_EXEC | VM_MAYEXEC);
	vma->vm_flags |= VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP |
		VM_DONTCOPY;

	if (remap_pfn_range(vma, vma->vm_start, slot->phys >> PAGE_SHIFT,
			    size, vma->vm_page_prot))
		return -EAGAIN;
	return 0;
}

static long scanout_slots_ioctl(struct file *file, unsigned int command,
				unsigned long argument)
{
	if (command != MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT)
		return -ENOTTY;
	if (copy_to_user((void __user *)argument, &scanout_slots_layout,
			 sizeof(scanout_slots_layout)))
		return -EFAULT;
	return 0;
}

static const struct file_operations scanout_slots_fops = {
	.owner = THIS_MODULE,
	.unlocked_ioctl = scanout_slots_ioctl,
	.mmap = scanout_slots_mmap,
	.llseek = no_llseek,
};

static struct miscdevice scanout_slots_device = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = DEVICE_NAME,
	.fops = &scanout_slots_fops,
	.mode = 0600,
};

static int scanout_slots_validate_platform(void)
{
	struct fb_info *info = registered_fb[0];
	struct device_node *fb_node;
	unsigned int i;
	int ret;

	if (strcmp(UTS_RELEASE, MISTER_MAGIK_PLATFORM_KERNEL_RELEASE))
		return -ENODEV;
	if (!of_machine_is_compatible(MISTER_MAGIK_PLATFORM_MACHINE))
		return -ENODEV;
	fb_node = of_find_compatible_node(NULL, NULL,
					  MISTER_MAGIK_PLATFORM_FB_COMPATIBLE);
	if (!fb_node)
		return -ENODEV;
	ret = of_address_to_resource(fb_node, 0, &framebuffer_resource);
	of_node_put(fb_node);
	if (ret ||
	    framebuffer_resource.start != MISTER_MAGIK_PLATFORM_FB_DT_BASE ||
	    resource_size(&framebuffer_resource) !=
		MISTER_MAGIK_PLATFORM_FB_DT_BYTES)
		return -ENODEV;
	if (!info || strcmp(info->fix.id, MISTER_MAGIK_PLATFORM_FB_ID))
		return -ENODEV;
	if (info->fix.smem_start != MISTER_MAGIK_PLATFORM_FB_VISIBLE_BASE)
		return -ENODEV;
	if (!info->fix.smem_len)
		return -ENODEV;
	if (!mister_magik_scanout_ranges_valid(info->fix.smem_start,
					       info->fix.smem_len,
					      scanout_slots[0].phys,
					      scanout_slots[1].phys,
					      RGB565_MAP_BYTES, ULONG_MAX))
		return -ENODEV;
	for (i = 0; i < ARRAY_SIZE(scanout_slots); i++) {
		if (region_intersects(scanout_slots[i].phys, RGB565_MAP_BYTES,
				      IORESOURCE_SYSTEM_RAM,
				      IORES_DESC_NONE) != REGION_DISJOINT)
			return -ENODEV;
	}
	return 0;
}

static void scanout_slots_release_resources(void)
{
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(scanout_slot_resources); i++) {
		if (scanout_slot_resources[i]) {
			release_mem_region(scanout_slots[i].phys, RGB565_MAP_BYTES);
			scanout_slot_resources[i] = NULL;
		}
	}
}

static void *scanout_slots_request_resource(unsigned long start,
					    unsigned long size,
					    unsigned int index,
					    void *context)
{
	(void)index;
	(void)context;
	return request_mem_region_exclusive(start, size, DEVICE_NAME);
}

static void scanout_slots_release_resource(unsigned long start,
					   unsigned long size,
					   void *resource,
					   unsigned int index,
					   void *context)
{
	(void)resource;
	(void)index;
	(void)context;
	release_mem_region(start, size);
}

static int scanout_slots_reserve_resources(void)
{
	static const unsigned long starts[] = {
		MISTER_MAGIK_PLATFORM_SLOT0_PHYS,
		MISTER_MAGIK_PLATFORM_SLOT1_PHYS,
	};

	return mister_magik_reserve_scanout_slots(starts, RGB565_MAP_BYTES,
		scanout_slot_resources, ARRAY_SIZE(starts),
		scanout_slots_request_resource, scanout_slots_release_resource,
		NULL);
}

static int __init scanout_slots_init(void)
{
	int ret;

	BUILD_BUG_ON(ARRAY_SIZE(scanout_slots) !=
		     MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT);
	BUILD_BUG_ON(sizeof(struct mister_magik_scanout_slots_layout) != 64);
	BUILD_BUG_ON(RGB565_MAP_BYTES >= REGION_OFFSET_BYTES);
	BUILD_BUG_ON(MISTER_MAGIK_PLATFORM_SLOT0_PHYS & ~PAGE_MASK);
	BUILD_BUG_ON(MISTER_MAGIK_PLATFORM_SLOT1_PHYS & ~PAGE_MASK);
	BUILD_BUG_ON(MISTER_MAGIK_PLATFORM_FRAME_BYTES !=
		     RGB565_STRIDE_BYTES * RGB565_HEIGHT);
	BUILD_BUG_ON(MISTER_MAGIK_PLATFORM_ABI_VERSION !=
		     MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION);

	ret = scanout_slots_validate_platform();
	if (ret) {
		pr_err("mister_magik_scanout_slots: unsupported framebuffer platform\n");
		return ret;
	}
	ret = scanout_slots_reserve_resources();
	if (ret) {
		pr_err("mister_magik_scanout_slots: scanout slots are not exclusively available\n");
		return ret;
	}

	ret = misc_register(&scanout_slots_device);
	if (ret) {
		scanout_slots_release_resources();
		return ret;
	}

	pr_info("mister_magik_scanout_slots: loaded /dev/%s ABI version=%u\n",
		DEVICE_NAME, MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION);
	return 0;
}

static void __exit scanout_slots_exit(void)
{
	misc_deregister(&scanout_slots_device);
	scanout_slots_release_resources();
	pr_info("mister_magik_scanout_slots: unloaded\n");
}

module_init(scanout_slots_init);
module_exit(scanout_slots_exit);

MODULE_DESCRIPTION("MiSTer MagiK write-combined scanout slot mappings");
MODULE_AUTHOR("MiSTer MagiK");
MODULE_LICENSE("GPL v2");
