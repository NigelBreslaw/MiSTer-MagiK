// SPDX-License-Identifier: GPL-2.0
/*
 * MiSTer MagiK write-combined framebuffer sidecar.
 *
 * This does not replace MiSTer_fb. It exposes the MiSTer hidden HPS framebuffer
 * slots used by SET_FBUF buffers 1 and 2 through a small misc device so
 * userspace can test whether non-live write-combined backbuffers fix
 * direct-render flicker.
 */

#include <linux/fs.h>
#include <linux/io.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/uaccess.h>

#define DEVICE_NAME "mister-magik-fbwc"
#define FB_ADDR 0x22000000UL
#define FB_SIZE_PX (1920UL * 1080UL)
#define FB_SLOT_BYTES (FB_SIZE_PX * 4UL)
#define FBWC_FIRST_BUFFER_INDEX 1UL
#define FBWC_BUFFER_COUNT 2UL
#define FBWC_PHYS_ADDR (FB_ADDR + (FB_SLOT_BYTES * FBWC_FIRST_BUFFER_INDEX))
#define FBWC_MAP_BYTES (FB_SLOT_BYTES * FBWC_BUFFER_COUNT)
#define FBWC_EXPECTED_KERNEL "5.15.1-MiSTer"

static atomic_t open_count = ATOMIC_INIT(0);
static atomic_t mmap_count = ATOMIC_INIT(0);

static int fbwc_open(struct inode *inode, struct file *file)
{
	atomic_inc(&open_count);
	return 0;
}

static int fbwc_release(struct inode *inode, struct file *file)
{
	atomic_dec(&open_count);
	return 0;
}

static void fbwc_vma_close(struct vm_area_struct *vma)
{
	atomic_dec(&mmap_count);
}

static const struct vm_operations_struct fbwc_vm_ops = {
	.close = fbwc_vma_close,
};

static int fbwc_mmap(struct file *file, struct vm_area_struct *vma)
{
	unsigned long size = vma->vm_end - vma->vm_start;
	unsigned long pfn = (FBWC_PHYS_ADDR >> PAGE_SHIFT) + vma->vm_pgoff;
	unsigned long max_pages = FBWC_MAP_BYTES >> PAGE_SHIFT;

	if (vma->vm_pgoff >= max_pages)
		return -EINVAL;
	if ((size >> PAGE_SHIFT) > (max_pages - vma->vm_pgoff))
		return -EINVAL;

	vma->vm_page_prot = pgprot_writecombine(vma->vm_page_prot);
	vma->vm_flags |= VM_IO | VM_DONTEXPAND | VM_DONTDUMP;

	if (io_remap_pfn_range(vma, vma->vm_start, pfn, size,
			       vma->vm_page_prot))
		return -EAGAIN;

	vma->vm_ops = &fbwc_vm_ops;
	atomic_inc(&mmap_count);
	return 0;
}

static ssize_t fbwc_read(struct file *file, char __user *buf, size_t len,
			 loff_t *ppos)
{
	char tmp[384];
	int n;

	n = scnprintf(tmp, sizeof(tmp),
		      "name=%s\nversion=2\nexpected_kernel=%s\nphys=0x%08lx\nmap_bytes=%lu\nbuffer_index=%lu\nbuffer_count=%lu\nslot_bytes=%lu\nopen_count=%d\nmmap_count=%d\n",
		      DEVICE_NAME, FBWC_EXPECTED_KERNEL, FBWC_PHYS_ADDR,
		      FBWC_MAP_BYTES, FBWC_FIRST_BUFFER_INDEX, FBWC_BUFFER_COUNT,
		      FB_SLOT_BYTES, atomic_read(&open_count),
		      atomic_read(&mmap_count));

	return simple_read_from_buffer(buf, len, ppos, tmp, n);
}

static const struct file_operations fbwc_fops = {
	.owner = THIS_MODULE,
	.open = fbwc_open,
	.release = fbwc_release,
	.read = fbwc_read,
	.mmap = fbwc_mmap,
	.llseek = no_llseek,
};

static struct miscdevice fbwc_miscdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = DEVICE_NAME,
	.fops = &fbwc_fops,
	.mode = 0600,
};

static int __init fbwc_init(void)
{
	int ret = misc_register(&fbwc_miscdev);

	if (ret)
		return ret;

	pr_info("mister_magik_fbwc: /dev/%s maps buffers %lu..%lu at phys 0x%08lx (%lu bytes)\n",
		DEVICE_NAME, FBWC_FIRST_BUFFER_INDEX,
		FBWC_FIRST_BUFFER_INDEX + FBWC_BUFFER_COUNT - 1, FBWC_PHYS_ADDR,
		FBWC_MAP_BYTES);
	return 0;
}

static void __exit fbwc_exit(void)
{
	misc_deregister(&fbwc_miscdev);
	pr_info("mister_magik_fbwc: unloaded\n");
}

module_init(fbwc_init);
module_exit(fbwc_exit);

MODULE_DESCRIPTION("MiSTer MagiK write-combined framebuffer sidecar");
MODULE_AUTHOR("MiSTer MagiK");
MODULE_LICENSE("GPL v2");
