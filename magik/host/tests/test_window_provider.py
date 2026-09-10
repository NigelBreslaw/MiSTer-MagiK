"""Execute the actual provider C with mocked kernel effects, not hardware."""

from pathlib import Path
import shutil
import subprocess

import pytest


def test_provider_lifecycle_and_mapping(tmp_path):
    compiler = shutil.which("cc")
    if not compiler:
        pytest.skip("C compiler unavailable")
    root = Path(__file__).resolve().parents[3]
    headers = [
        "fs",
        "ioport",
        "miscdevice",
        "mm",
        "module",
        "of",
        "string",
        "uaccess",
        "types",
        "ioctl",
    ]
    for name in headers:
        path = tmp_path / "linux" / f"{name}.h"
        path.parent.mkdir(exist_ok=True)
        path.write_text('#include "fake.h"\n')
    (tmp_path / "generated").mkdir()
    (tmp_path / "generated/utsrelease.h").write_text(
        '#define UTS_RELEASE "6.18.38-MiSTer"\n'
    )
    (tmp_path / "fake.h").write_text(r"""
#ifndef FAKE_H
#define FAKE_H
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
typedef uint32_t __u32;
#define _IOR(c,n,t) ((unsigned int)(sizeof(t)<<16)|(c<<8)|n)
#define CONFIG_ARM 1
#define CONFIG_ARM_LPAE 0
#define CONFIG_IO_STRICT_DEVMEM 0
#define IS_ENABLED(x) (x)
#define PAGE_SIZE 4096UL
#define PAGE_SHIFT 12
#define L_PTE_MT_MASK 0x3cUL
#define L_PTE_SHARED 0x400UL
#define L_PTE_XN 0x200UL
#define pgprot_val(p) (p)
#define VM_SHARED 1UL
#define VM_READ 2UL
#define VM_WRITE 4UL
#define VM_EXEC 8UL
#define VM_MAYEXEC 16UL
#define VM_IO 32UL
#define VM_PFNMAP 64UL
#define VM_DONTEXPAND 128UL
#define VM_DONTDUMP 256UL
#define VM_DONTCOPY 512UL
#define __user
#define THIS_MODULE ((void *)1)
#define MISC_DYNAMIC_MINOR 255
#define __init
#define __exit
#define module_init(...)
#define module_exit(...)
#define MODULE_DESCRIPTION(...)
#define MODULE_LICENSE(...)
#define MODULE_INFO(...)
#define BUILD_BUG_ON(x) _Static_assert(!(x), "build assertion")
#define smp_load_acquire(p) (*(p))
#define smp_store_release(p,v) (*(p)=(v))
struct inode { int unused; };
struct file { int unused; };
struct vm_area_struct { unsigned long vm_start, vm_end, vm_pgoff, vm_flags, vm_page_prot; };
struct file_operations {
    void *owner;
    int (*open)(struct inode *, struct file *);
    int (*mmap)(struct file *, struct vm_area_struct *);
    long (*unlocked_ioctl)(struct file *, unsigned int, unsigned long);
    void *llseek;
};
struct miscdevice { int minor; const char *name; const struct file_operations *fops; int mode; };
static int board=1, valid_ram, claim_fail, register_fail, map_fail, copy_fail;
static int claims, releases, registrations, deregistrations, maps;
static unsigned long mapped_phys;
static unsigned long lookup_bad_page;
static int lookup_fault;
static unsigned long lookups, lookup_ends;
static bool lookup_held;
struct follow_pfnmap_args {
    struct vm_area_struct *vma;
    unsigned long address, pfn, pgprot;
    bool writable;
};
static int follow_pfnmap_start(struct follow_pfnmap_args *a) {
    assert(!lookup_held);
    unsigned long page=(a->address-a->vma->vm_start)/PAGE_SIZE;
    lookups++;
    if(page==lookup_bad_page && lookup_fault==1) return -ENOENT;
    lookup_held=true;
    a->pfn=(mapped_phys>>PAGE_SHIFT)+page;
    a->pgprot=a->vma->vm_page_prot;
    a->writable=true;
    if(page==lookup_bad_page) {
        if(lookup_fault==2) a->pfn++;
        if(lookup_fault==3) a->pgprot^=4;
        if(lookup_fault==4) a->pgprot^=L_PTE_SHARED;
        if(lookup_fault==5) a->pgprot^=L_PTE_XN;
        if(lookup_fault==6) a->writable=false;
        if(lookup_fault==7) a->pgprot^=0x8000; /* unrelated PTE state */
    }
    return 0;
}
static void follow_pfnmap_end(struct follow_pfnmap_args *a) {
    assert(lookup_held); lookup_held=false; lookup_ends++;
    memset(a,0,sizeof(*a)); /* results invalid after unlock */
}
static bool held;
static int of_machine_is_compatible(const char *s) { assert(!strcmp(s,"altr,socfpga-cyclone5")); return board; }
static int pfn_valid(unsigned long pfn) { assert(pfn>=0x22000 && pfn<0x237bb); return valid_ram; }
static void *request_mem_region_exclusive(unsigned long p, unsigned long n, const char *s) {
    assert(p==0x22000000 && n==0x17bb000 && s); claims++;
    held=!claim_fail; return held ? (void *)1 : 0;
}
static void release_mem_region(unsigned long p, unsigned long n) { assert(held && p==0x22000000 && n==0x17bb000); held=false; releases++; }
static int misc_register(struct miscdevice *d) {
    assert(held && d->mode==0600 && d->fops->owner==THIS_MODULE);
    assert(d->fops->open(0,0)==-ENODEV); /* no access before both nodes */
    registrations++; return registrations==register_fail ? -EIO : 0;
}
static void misc_deregister(struct miscdevice *d) { assert(held); assert(d->fops->open(0,0)==-ENODEV); deregistrations++; }
static unsigned long pgprot_writecombine(unsigned long p) { (void)p; return 0x123; }
static void vm_flags_init(struct vm_area_struct *v, unsigned long f) { v->vm_flags = f; }
static int remap_pfn_range(struct vm_area_struct *v, unsigned long start, unsigned long pfn, unsigned long size, unsigned long prot) {
    assert(start==v->vm_start && size==v->vm_end-v->vm_start && prot==0x123);
    assert(!(v->vm_flags&(VM_EXEC|VM_MAYEXEC)));
    assert((v->vm_flags&(VM_IO|VM_PFNMAP|VM_DONTEXPAND|VM_DONTDUMP|VM_DONTCOPY))==992);
    maps++; mapped_phys=pfn<<12; return map_fail;
}
static int copy_to_user(void *to, const void *from, unsigned long n) { if(copy_fail) return 1; memcpy(to,from,n); return 0; }
#endif
""")
    source = tmp_path / "test.c"
    source.write_text(
        f'#include "{root}/mister/platform/kernel/scanout-618/provider.c"\n'
        f'#include "{root}/mister/platform/kernel/scanout-618/entry.c"\n'
        + r"""
static void reset(void) {
    board=1; valid_ram=claim_fail=register_fail=map_fail=copy_fail=0;
    claims=releases=registrations=deregistrations=maps=0;
    lookup_fault=lookups=lookup_ends=0; lookup_bad_page=0;
    assert(!lookup_held);
    assert(!held && !claimed && !ready);
}
int main(void) {
    reset();
    assert(window_provider_init()==-EOPNOTSUPP && !claims);
    window_provider_exit(); assert(!releases);
    assert(mister_magik_window_provider_register(false)==-EOPNOTSUPP && !claims);
    board=0; assert(mister_magik_window_provider_register(true)==-ENODEV && !claims);
    board=1; valid_ram=1; assert(mister_magik_window_provider_register(true)==-EPERM && !claims);
    valid_ram=0; claim_fail=1; assert(mister_magik_window_provider_register(true)==-EBUSY && !releases);
    for(int fail=1; fail<=2; fail++) {
        reset(); register_fail=fail;
        assert(mister_magik_window_provider_register(true)==-EIO);
        assert(registrations==fail && deregistrations==fail-1 && releases==1 && !held && !ready);
        mister_magik_window_provider_unregister(); assert(releases==1);
    }
    reset(); assert(mister_magik_window_provider_register(true)==0);
    assert(claims==1 && registrations==2 && held && ready && window_open(0,0)==0);
    assert(mister_magik_window_provider_register(true)==-EBUSY && claims==1);
    for(unsigned int flags=0; flags<16; flags++) {
        struct vm_area_struct v={0x1000,0x1000+0x17bb000,0,flags|VM_MAYEXEC,0};
        assert(main_mmap(0,&v)==(flags==7?0:-EINVAL));
    }
    assert(maps==1 && mapped_phys==0x22000000);
    for(int slot=0; slot<2; slot++) {
        struct vm_area_struct v={0x1000,0x1000+SLOT_BYTES,slot?2025:0,7|VM_MAYEXEC,0};
        assert(slots_mmap(0,&v)==0 && mapped_phys==(slot?SLOT1_BASE:SLOT0_BASE));
        v.vm_pgoff=1; assert(slots_mmap(0,&v)==-EINVAL);
        v.vm_pgoff=0; v.vm_end--; assert(slots_mmap(0,&v)==-EINVAL);
    }
    struct vm_area_struct v={0x1000,0x1000+0x17bb000,1,7,0};
    assert(main_mmap(0,&v)==-EINVAL); v.vm_pgoff=0; map_fail=1;
    assert(main_mmap(0,&v)==-EAGAIN);
    map_fail=0;
    /* Each failure position must stop the walk and release every acquired
     * lookup, including first/last pages. No end follows a failed start.
     */
    for(unsigned long page=0; page<0x17bb000/PAGE_SIZE; page+=0x17bb000/PAGE_SIZE-1) {
        lookup_bad_page=page;
        for(int fault=1; fault<=7; fault++) {
            lookup_fault=fault; lookups=lookup_ends=0;
            assert(main_mmap(0,&v)==(fault==1?-ENOENT:fault==7?0:-EIO));
            assert(!lookup_held);
            assert(lookups==(fault==7?0x17bb000/PAGE_SIZE:page+1));
            assert(lookup_ends==lookups-(fault==1));
        }
    }
    lookup_fault=0;
    struct mister_magik_main_window_layout layout;
    assert(main_ioctl(0,0,(unsigned long)&layout)==-ENOTTY);
    assert(main_ioctl(0,MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT,(unsigned long)&layout)==0);
    assert(mister_magik_main_window_layout_valid(&layout));
    struct mister_magik_scanout_slots_layout slots;
    assert(slots_ioctl(0,MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT,(unsigned long)&slots)==-ENOTTY);
    assert(slots_ioctl(0,MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT,(unsigned long)&slots)==0);
    assert(slots.abi_version==3 && slots.slots[1].physical_address==SLOT1_BASE && slots.reserved[3]==0);
    copy_fail=1; assert(main_ioctl(0,MISTER_MAGIK_MAIN_WINDOW_GET_LAYOUT,0)==-EFAULT);
    assert(slots_ioctl(0,MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT,0)==-EFAULT);
    mister_magik_window_provider_unregister();
    assert(!ready && !held && releases==1 && deregistrations==2);
    mister_magik_window_provider_unregister(); assert(releases==1);
    return 0;
}
"""
    )
    binary = tmp_path / "provider-test"
    subprocess.run(
        [
            compiler,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-unused-parameter",
            "-I",
            str(tmp_path),
            str(source),
            "-o",
            str(binary),
        ],
        check=True,
    )
    subprocess.run([str(binary)], check=True)
