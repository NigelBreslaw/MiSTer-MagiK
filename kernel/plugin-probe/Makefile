KERNEL_SRC ?= ../../reference/Linux-Kernel_MiSTer
KERNEL_BUILD ?=
ARCH ?= arm
CROSS_COMPILE ?= arm-linux-gnueabihf-
LOCALVERSION ?= -MiSTer

obj-m += mister_magik_plugin_probe.o
ccflags-y += -fno-stack-protector

.PHONY: all clean

ifneq ($(KERNEL_BUILD),)
KERNEL_OUTPUT := O=$(KERNEL_BUILD)
endif

all:
	$(MAKE) -C $(KERNEL_SRC) $(KERNEL_OUTPUT) M=$(CURDIR) ARCH=$(ARCH) CROSS_COMPILE=$(CROSS_COMPILE) LOCALVERSION=$(LOCALVERSION) modules

clean:
	$(MAKE) -C $(KERNEL_SRC) $(KERNEL_OUTPUT) M=$(CURDIR) ARCH=$(ARCH) CROSS_COMPILE=$(CROSS_COMPILE) LOCALVERSION=$(LOCALVERSION) clean
