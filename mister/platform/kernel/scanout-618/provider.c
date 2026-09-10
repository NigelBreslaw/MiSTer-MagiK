// SPDX-License-Identifier: GPL-2.0-only
// Copyright (C) 2026 Nigel Breslaw
#include <linux/fs.h>
#include <linux/ioport.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>
#include "provider.h"
#include "../main-window/mister_magik_main_window_layout.h"
#include "../scanout-slots/mister_magik_scanout_slots_uapi.h"

#define SLOT_BYTES 2101248UL
#define SLOT0_BASE 0x227e9000UL
#define SLOT1_BASE 0x22fd2000UL
#define SLOT1_OFFSET 8294400UL

static bool ready;
static bool claimed;
static const struct mister_magik_main_window_layout main_layout =
	MISTER_MAGIK_MAIN_WINDOW_LAYOUT_INITIALIZER;
static const struct mister_magik_scanout_slots_layout slot_layout = {
	.abi_version = MISTER_MAGIK_SCANOUT_SLOTS_ABI_VERSION,
	.slot_count = 2, .max_width = 1366, .max_height = 768,
	.max_stride_bytes = 2736, .slot_capacity_bytes = SLOT_BYTES,
	.map_bytes = SLOT_BYTES,
	.flags = MISTER_MAGIK_SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE,
	.slots = {{SLOT0_BASE, 0}, {SLOT1_BASE, SLOT1_OFFSET}},
};

static int window_open(struct inode *inode, struct file *file)
{
	/* Neither node can be used during partial registration or rollback. */
	return smp_load_acquire(&ready) ? 0 : -ENODEV;
}

static int map_fixed(struct vm_area_struct *vma, unsigned long physical)
{
	vma->vm_page_prot = pgprot_writecombine(vma->vm_page_prot);
	/* Only called from our initial mmap callbacks. In the pinned 6.18
	 * __mmap_new_vma path, these run before vma_iter_store_new: the VMA is
	 * not yet in the tree. vm_flags_init is the documented API for that
	 * state. Do not reuse this helper to modify an existing VMA.
	 */
	vm_flags_init(vma, (vma->vm_flags & ~(VM_EXEC | VM_MAYEXEC)) |
		VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP | VM_DONTCOPY);
	return remap_pfn_range(vma, vma->vm_start, physical >> PAGE_SHIFT,
		vma->vm_end - vma->vm_start, vma->vm_page_prot) ? -EAGAIN : 0;
}

static int main_mmap(struct file *file, struct vm_area_struct *vma)
{
	if (!mister_magik_main_window_mapping_valid(vma->vm_pgoff,
		vma->vm_end - vma->vm_start, PAGE_SIZE,
		vma->vm_flags & VM_SHARED, vma->vm_flags & VM_READ,
		vma->vm_flags & VM_WRITE, vma->vm_flags & VM_EXEC))
		return -EINVAL;
	return map_fixed(vma, MISTER_MAGIK_MAIN_WINDOW_BASE);
}

static int slots_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long physical;
	if (vma->vm_end - vma->vm_start != SLOT_BYTES ||
	    !(vma->vm_flags & VM_SHARED) || !(vma->vm_flags & VM_READ) ||
	    !(vma->vm_flags & VM_WRITE) || (vma->vm_flags & VM_EXEC))
		return -EINVAL;
	if (vma->vm_pgoff == 0)
		physical = SLOT0_BASE;
	else if (vma->vm_pgoff == SLOT1_OFFSET / PAGE_SIZE)
		physical = SLOT1_BASE;
	else
		return -EINVAL;
	return map_fixed(vma, physical);
}

static long main_ioctl(struct file *file, unsigned int command, unsigned long arg)
{
	if (command != MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT)
		return -ENOTTY;
	return copy_to_user((void __user *)arg, &main_layout, sizeof(main_layout)) ? -EFAULT : 0;
}

static long slots_ioctl(struct file *file, unsigned int command, unsigned long arg)
{
	if (command != MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT)
		return -ENOTTY;
	return copy_to_user((void __user *)arg, &slot_layout, sizeof(slot_layout)) ? -EFAULT : 0;
}

static const struct file_operations main_fops = {
	.owner = THIS_MODULE, .open = window_open,
	.mmap = main_mmap, .unlocked_ioctl = main_ioctl,
};
static const struct file_operations slots_fops = {
	.owner = THIS_MODULE, .open = window_open,
	.mmap = slots_mmap, .unlocked_ioctl = slots_ioctl,
};
static struct miscdevice main_device = {
	.minor = MISC_DYNAMIC_MINOR, .name = "mister-magik-main-window",
	.fops = &main_fops, .mode = 0600,
};
static struct miscdevice slots_device = {
	.minor = MISC_DYNAMIC_MINOR, .name = "mister-magik-scanout-slots",
	.fops = &slots_fops, .mode = 0600,
};

int mister_magik_window_provider_register(bool platform_qualified)
{
	unsigned long physical;
	int result;
	BUILD_BUG_ON(sizeof(main_layout) != 64 || sizeof(slot_layout) != 64);
	BUILD_BUG_ON(SLOT0_BASE < MISTER_MAGIK_MAIN_WINDOW_BASE);
	BUILD_BUG_ON(SLOT0_BASE + SLOT_BYTES > SLOT1_BASE);
	BUILD_BUG_ON(SLOT1_BASE + SLOT_BYTES > MISTER_MAGIK_MAIN_WINDOW_BASE + MISTER_MAGIK_MAIN_WINDOW_BYTES);
	BUILD_BUG_ON(SLOT1_OFFSET % 4096 || SLOT_BYTES % 4096);
	if (!platform_qualified)
		return -EOPNOTSUPP;
	if (claimed)
		return -EBUSY;
	if (!IS_ENABLED(CONFIG_ARM) || IS_ENABLED(CONFIG_ARM_LPAE) ||
	    IS_ENABLED(CONFIG_IO_STRICT_DEVMEM) || PAGE_SIZE != 4096)
		return -ENODEV;
	if (strcmp(UTS_RELEASE, "6.18.38-MiSTer"))
		return -ENODEV;
	if (!of_machine_is_compatible("altr,socfpga-cyclone5"))
		return -ENODEV;
	for (physical = MISTER_MAGIK_MAIN_WINDOW_BASE;
	     physical < MISTER_MAGIK_MAIN_WINDOW_BASE + MISTER_MAGIK_MAIN_WINDOW_BYTES;
	     physical += PAGE_SIZE)
		if (pfn_valid(physical >> PAGE_SHIFT))
			return -EPERM;
	if (!request_mem_region_exclusive(MISTER_MAGIK_MAIN_WINDOW_BASE,
		MISTER_MAGIK_MAIN_WINDOW_BYTES, "mister-magik-window-provider"))
		return -EBUSY;
	claimed = true;
	result = misc_register(&slots_device);
	if (result)
		goto release;
	result = misc_register(&main_device);
	if (result) {
		misc_deregister(&slots_device);
		goto release;
	}
	smp_store_release(&ready, true);
	return 0;
release:
	release_mem_region(MISTER_MAGIK_MAIN_WINDOW_BASE, MISTER_MAGIK_MAIN_WINDOW_BYTES);
	claimed = false;
	return result;
}

void mister_magik_window_provider_unregister(void)
{
	if (!claimed)
		return;
	smp_store_release(&ready, false);
	misc_deregister(&main_device);
	misc_deregister(&slots_device);
	/* Normal module unload is blocked by fops ownership, including VMA-held
	 * file references after the userspace fd closes. Never force unload.
	 */
	release_mem_region(MISTER_MAGIK_MAIN_WINDOW_BASE, MISTER_MAGIK_MAIN_WINDOW_BYTES);
	claimed = false;
}
