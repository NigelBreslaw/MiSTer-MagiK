// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK production scanout-slot mappings.
 *
 * Exposes exactly two framebuffer-reserved hidden RGB565 slots through one
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
#include <linux/string.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>

#include "mister_magik_scanout_slots_uapi.h"

#define DEVICE_NAME "mister-magik-scanout-slots"
#define FB_PHYS_BASE 0x22000000UL
#define FB_VISIBLE_PHYS_BASE (FB_PHYS_BASE + PAGE_SIZE)
#define RGB565_WIDTH 960UL
#define RGB565_HEIGHT 540UL
#define RGB565_STRIDE_BYTES 1920UL
#define RGB565_FRAME_BYTES (RGB565_STRIDE_BYTES * RGB565_HEIGHT)
#define RGB565_MAP_BYTES PAGE_ALIGN(RGB565_FRAME_BYTES)
#define HIDDEN_SLOT_BYTES (1920UL * 1080UL * 4UL)

#define REGION_OFFSET_PAGES 256UL
#define REGION_OFFSET_BYTES (REGION_OFFSET_PAGES * PAGE_SIZE)

struct scanout_slot {
	unsigned long phys;
	unsigned long mmap_pgoff;
};

static const struct scanout_slot scanout_slots[] = {
	{
		.phys = FB_PHYS_BASE + HIDDEN_SLOT_BYTES,
		.mmap_pgoff = 0,
	},
	{
		.phys = FB_PHYS_BASE + (HIDDEN_SLOT_BYTES * 2UL),
		.mmap_pgoff = REGION_OFFSET_PAGES,
	},
};

static struct resource *scanout_slot_resources[MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT];
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
			.physical_address = FB_PHYS_BASE + HIDDEN_SLOT_BYTES,
			.mmap_offset_bytes = 0,
		},
		{
			.physical_address = FB_PHYS_BASE +
				(HIDDEN_SLOT_BYTES * 2UL),
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
	resource_size_t visible_end;
	int ret;

	if (strcmp(UTS_RELEASE, "5.15.1-MiSTer"))
		return -ENODEV;
	if (!of_machine_is_compatible("altr,socfpga-cyclone5"))
		return -ENODEV;
	fb_node = of_find_compatible_node(NULL, NULL, "MiSTer_fb");
	if (!fb_node)
		return -ENODEV;
	ret = of_address_to_resource(fb_node, 0, &framebuffer_resource);
	of_node_put(fb_node);
	if (ret || framebuffer_resource.start != FB_PHYS_BASE ||
	    resource_size(&framebuffer_resource) < HIDDEN_SLOT_BYTES)
		return -ENODEV;
	if (!info || strcmp(info->fix.id, "MiSTer_fb"))
		return -ENODEV;
	if (info->fix.smem_start != FB_PHYS_BASE &&
	    info->fix.smem_start != FB_VISIBLE_PHYS_BASE)
		return -ENODEV;
	if (!info->fix.smem_len)
		return -ENODEV;
	visible_end = info->fix.smem_start + info->fix.smem_len;
	if (visible_end < info->fix.smem_start ||
	    visible_end > scanout_slots[0].phys)
		return -ENODEV;
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

static int scanout_slots_reserve_resources(void)
{
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(scanout_slots); i++) {
		scanout_slot_resources[i] =
			request_mem_region(scanout_slots[i].phys,
					   RGB565_MAP_BYTES, DEVICE_NAME);
		if (!scanout_slot_resources[i]) {
			scanout_slots_release_resources();
			return -EBUSY;
		}
	}
	return 0;
}

static int __init scanout_slots_init(void)
{
	int ret;

	BUILD_BUG_ON(ARRAY_SIZE(scanout_slots) !=
		     MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT);
	BUILD_BUG_ON(sizeof(struct mister_magik_scanout_slots_layout) != 64);
	BUILD_BUG_ON(RGB565_MAP_BYTES >= REGION_OFFSET_BYTES);
	BUILD_BUG_ON((FB_PHYS_BASE + HIDDEN_SLOT_BYTES) & ~PAGE_MASK);
	BUILD_BUG_ON((FB_PHYS_BASE + (HIDDEN_SLOT_BYTES * 2UL)) & ~PAGE_MASK);

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
