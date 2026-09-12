// SPDX-License-Identifier: GPL-2.0-only
// Copyright (C) 2026 Nigel Breslaw
#include <linux/fs.h>
#include <linux/ioport.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <generated/utsrelease.h>
#include "provider.h"
#include "mister_magik_mapping_diagnostic_uapi.h"
#include "../main-window/mister_magik_main_window_layout.h"
#include "../scanout-slots/mister_magik_scanout_slots_uapi.h"

#define SLOT_BYTES 2101248UL
#define SLOT0_BASE 0x227e9000UL
#define SLOT1_BASE 0x22fd2000UL
#define SLOT1_OFFSET 8294400UL

static bool ready;
static bool claimed;
struct mapping_file_context {
	spinlock_t lock;
	struct mister_magik_mapping_diagnostic diagnostic;
};
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
	struct mapping_file_context *context;

	/* Neither node can be used during partial registration or rollback. */
	if (!smp_load_acquire(&ready))
		return -ENODEV;
	context = kzalloc(sizeof(*context), GFP_KERNEL);
	if (!context)
		return -ENOMEM;
	spin_lock_init(&context->lock);
	context->diagnostic.abi_version = MISTER_MAGIK_MAPPING_DIAGNOSTIC_ABI_VERSION;
	context->diagnostic.record_bytes = sizeof(context->diagnostic);
	context->diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_NOT_ATTEMPTED;
	context->diagnostic.page_index = MISTER_MAGIK_MAPPING_DIAGNOSTIC_NO_PAGE;
	file->private_data = context;
	return 0;
}

static int window_release(struct inode *inode, struct file *file)
{
	kfree(file->private_data);
	file->private_data = NULL;
	return 0;
}

static void diagnostic_store(struct mapping_file_context *context,
	const struct mister_magik_mapping_diagnostic *diagnostic)
{
	unsigned long flags;

	spin_lock_irqsave(&context->lock, flags);
	context->diagnostic = *diagnostic;
	spin_unlock_irqrestore(&context->lock, flags);
}

static struct mister_magik_mapping_diagnostic diagnostic_begin(
	struct file *file, unsigned long physical, unsigned long map_bytes,
	unsigned long protection_mask, unsigned long expected_protection)
{
	struct mister_magik_mapping_diagnostic diagnostic = {
		.abi_version = MISTER_MAGIK_MAPPING_DIAGNOSTIC_ABI_VERSION,
		.record_bytes = sizeof(diagnostic),
		.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_IN_PROGRESS,
		.page_index = MISTER_MAGIK_MAPPING_DIAGNOSTIC_NO_PAGE,
		.protection_mask = protection_mask,
		.expected_protection = expected_protection,
		.physical_base = physical,
		.map_bytes = map_bytes,
	};

	diagnostic_store(file->private_data, &diagnostic);
	return diagnostic;
}

static long diagnostic_ioctl(struct file *file, unsigned long arg)
{
	struct mapping_file_context *context = file->private_data;
	struct mister_magik_mapping_diagnostic diagnostic;
	unsigned long flags;

	spin_lock_irqsave(&context->lock, flags);
	diagnostic = context->diagnostic;
	spin_unlock_irqrestore(&context->lock, flags);
	return copy_to_user((void __user *)arg, &diagnostic, sizeof(diagnostic)) ?
		-EFAULT : 0;
}

static unsigned int mapping_protection_failures(pgprot_t protection,
	unsigned int *verification_flags)
{
	const unsigned long value = pgprot_val(protection);
	unsigned int failures = 0;

	if ((value & L_PTE_MT_MASK) == L_PTE_MT_BUFFERABLE)
		*verification_flags |= MISTER_MAGIK_MAPPING_VERIFIED_WC;
	else
		failures |= MISTER_MAGIK_MAPPING_FAILURE_WC;
	if (value & L_PTE_XN)
		*verification_flags |= MISTER_MAGIK_MAPPING_VERIFIED_XN;
	else
		failures |= MISTER_MAGIK_MAPPING_FAILURE_XN;
	if (!(value & L_PTE_RDONLY))
		*verification_flags |=
			MISTER_MAGIK_MAPPING_VERIFIED_WRITABLE_PROTECTION;
	else
		failures |= MISTER_MAGIK_MAPPING_FAILURE_READONLY;
	if (value & L_PTE_SHARED)
		*verification_flags |= MISTER_MAGIK_MAPPING_VERIFIED_SHARED;
	else
		failures |= MISTER_MAGIK_MAPPING_FAILURE_SHARED;
	return failures;
}

static int verify_mapping(struct file *file, struct vm_area_struct *vma,
	unsigned long physical, struct mister_magik_mapping_diagnostic diagnostic)
{
	unsigned long address;
	/* Called only during initial mmap, with the caller's mmap write lock
	 * held. Inspect every installed Linux PTE through the supported API;
	 * never touch the mapped memory or retain lookup fields after end().
	 * This does not inspect ARM hardware PTEs/PRRR/NMRR, other aliases,
	 * future mprotect changes, or establish CPU/FPGA ownership.
	 */
	for (address = vma->vm_start; address < vma->vm_end;
	     address += PAGE_SIZE) {
		struct follow_pfnmap_args args = { .vma = vma, .address = address };
		unsigned long expected_pfn =
			(physical + address - vma->vm_start) >> PAGE_SHIFT;
		unsigned long observed_pfn;
		unsigned int failures = 0;
		bool writable;
		int result = follow_pfnmap_start(&args);
		if (result) {
			diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_FAILED;
			diagnostic.failure_flags = MISTER_MAGIK_MAPPING_FAILURE_LOOKUP;
			diagnostic.page_index =
				(address - vma->vm_start) >> PAGE_SHIFT;
			diagnostic.expected_pfn = expected_pfn;
			diagnostic.error_code = result;
			diagnostic_store(file->private_data, &diagnostic);
			return result;
		}
		observed_pfn = args.pfn;
		writable = args.writable;
		follow_pfnmap_end(&args);
		if (observed_pfn != expected_pfn)
			failures |= MISTER_MAGIK_MAPPING_FAILURE_PFN;
		if (!writable)
			failures |= MISTER_MAGIK_MAPPING_FAILURE_WRITABLE;
		if (failures) {
			diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_FAILED;
			diagnostic.failure_flags = failures;
			diagnostic.page_index =
				(address - vma->vm_start) >> PAGE_SHIFT;
			diagnostic.expected_pfn = expected_pfn;
			diagnostic.observed_pfn = observed_pfn;
			diagnostic.writable = writable;
			diagnostic.error_code = -EIO;
			diagnostic_store(file->private_data, &diagnostic);
			return -EIO;
		}
	}
	diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_PASSED;
	diagnostic.page_index = MISTER_MAGIK_MAPPING_DIAGNOSTIC_NO_PAGE;
	diagnostic.verification_flags |= MISTER_MAGIK_MAPPING_VERIFIED_PFNS |
		MISTER_MAGIK_MAPPING_VERIFIED_WRITABILITY;
	diagnostic_store(file->private_data, &diagnostic);
	return 0;
}

static int map_fixed(struct file *file, struct vm_area_struct *vma,
	unsigned long physical)
{
	struct mister_magik_mapping_diagnostic diagnostic;
	vm_flags_t mapping_flags;
	pgprot_t protection;
	int result;

	mapping_flags = (vma->vm_flags & ~(VM_EXEC | VM_MAYEXEC)) |
		VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP | VM_DONTCOPY;
	/* The pinned 6.18 __mmap_new_file_vma path invokes this callback before
	 * inserting the VMA into the tree. vm_flags_init() is the supported API
	 * for that state. Generate the protection from the finalized flags, add
	 * ARM write-combining, and pass this exact value to remap_pfn_range().
	 */
	vm_flags_init(vma, mapping_flags);
	protection = pgprot_writecombine(vm_get_page_prot(mapping_flags));
	vma->vm_page_prot = protection;
	diagnostic = diagnostic_begin(file, physical,
		vma->vm_end - vma->vm_start,
		L_PTE_MT_MASK | L_PTE_SHARED | L_PTE_XN,
		pgprot_val(protection));
	diagnostic.verification_flags = MISTER_MAGIK_MAPPING_VERIFIED_VMA_FLAGS |
		MISTER_MAGIK_MAPPING_PROTECTION_READBACK_UNAVAILABLE;
	diagnostic.failure_flags = mapping_protection_failures(protection,
		&diagnostic.verification_flags);
	if (diagnostic.failure_flags) {
		diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_FAILED;
		diagnostic.error_code = -EIO;
		diagnostic_store(file->private_data, &diagnostic);
		return -EIO;
	}
	diagnostic.verification_flags |=
		MISTER_MAGIK_MAPPING_VERIFIED_PROTECTION_SUPPLIED;
	result = remap_pfn_range(vma, vma->vm_start, physical >> PAGE_SHIFT,
		vma->vm_end - vma->vm_start, protection);
	if (result) {
		diagnostic.state = MISTER_MAGIK_MAPPING_DIAGNOSTIC_FAILED;
		diagnostic.failure_flags = MISTER_MAGIK_MAPPING_FAILURE_REMAP;
		diagnostic.error_code = result;
		diagnostic_store(file->private_data, &diagnostic);
		return -EAGAIN;
	}
	/* On failure the mmap core tears down this unsuccessful mapping. */
	return verify_mapping(file, vma, physical, diagnostic);
}

static int main_mmap(struct file *file, struct vm_area_struct *vma)
{
	/* Invalid requests must not leave a previous mapping's evidence visible. */
	diagnostic_begin(file, 0, 0, 0, 0);
	if (!mister_magik_main_window_mapping_valid(vma->vm_pgoff,
		vma->vm_end - vma->vm_start, PAGE_SIZE,
		vma->vm_flags & VM_SHARED, vma->vm_flags & VM_READ,
		vma->vm_flags & VM_WRITE, vma->vm_flags & VM_EXEC))
		return -EINVAL;
	return map_fixed(file, vma, MISTER_MAGIK_MAIN_WINDOW_BASE);
}

static int slots_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long physical;
	diagnostic_begin(file, 0, 0, 0, 0);
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
	return map_fixed(file, vma, physical);
}

static long main_ioctl(struct file *file, unsigned int command, unsigned long arg)
{
	if (command == MISTER_MAGIK_MAPPING_GET_DIAGNOSTIC)
		return diagnostic_ioctl(file, arg);
	if (command != MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT)
		return -ENOTTY;
	return copy_to_user((void __user *)arg, &main_layout, sizeof(main_layout)) ? -EFAULT : 0;
}

static long slots_ioctl(struct file *file, unsigned int command, unsigned long arg)
{
	if (command == MISTER_MAGIK_MAPPING_GET_DIAGNOSTIC)
		return diagnostic_ioctl(file, arg);
	if (command != MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT)
		return -ENOTTY;
	return copy_to_user((void __user *)arg, &slot_layout, sizeof(slot_layout)) ? -EFAULT : 0;
}

static const struct file_operations main_fops = {
	.owner = THIS_MODULE, .open = window_open,
	.release = window_release, .mmap = main_mmap, .unlocked_ioctl = main_ioctl,
};
static const struct file_operations slots_fops = {
	.owner = THIS_MODULE, .open = window_open,
	.release = window_release, .mmap = slots_mmap, .unlocked_ioctl = slots_ioctl,
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
	BUILD_BUG_ON(sizeof(struct mister_magik_mapping_diagnostic) != 64);
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
