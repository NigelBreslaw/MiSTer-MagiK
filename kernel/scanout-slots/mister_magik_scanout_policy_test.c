// SPDX-License-Identifier: GPL-2.0
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <errno.h>
#include <limits.h>

#include "mister_magik_scanout_policy.h"

struct fixture {
	int fail_index;
	unsigned int requested;
	unsigned int released;
};

static void *request_slot(unsigned long start, unsigned long size,
			  unsigned int index, void *context)
{
	struct fixture *fixture = context;

	(void)start;
	(void)size;
	fixture->requested++;
	return (int)index == fixture->fail_index ? NULL : (void *)(unsigned long)(index + 1);
}

static void release_slot(unsigned long start, unsigned long size, void *resource,
			 unsigned int index, void *context)
{
	struct fixture *fixture = context;

	(void)start;
	(void)size;
	(void)resource;
	(void)index;
	fixture->released++;
}

int main(void)
{
	const unsigned long starts[] = { 0x227e9000UL, 0x22fd2000UL };
	void *resources[2] = { NULL, NULL };
	struct fixture fixture = { .fail_index = -1 };
	const unsigned long map = 1040384UL;

	assert(mister_magik_scanout_ranges_valid(0x22001000UL, map,
		starts[0], starts[1], map, 0xffffffffUL));
	assert(!mister_magik_scanout_ranges_valid(0x22001000UL, 0,
		starts[0], starts[1], map, 0xffffffffUL));
	assert(!mister_magik_scanout_ranges_valid(0x22001000UL,
		starts[0] - 0x22001000UL + 1, starts[0], starts[1], map,
		0xffffffffUL));
	assert(!mister_magik_scanout_ranges_valid(0x22001000UL, map,
		starts[0], starts[0] + map - 1, map, 0xffffffffUL));
	assert(!mister_magik_scanout_ranges_valid(0x22001000UL, map,
		starts[0], ULONG_MAX - map + 2, map, ULONG_MAX));
	assert(mister_magik_reserve_scanout_slots(starts, map, resources, 2,
		request_slot, release_slot, &fixture) == 0);
	fixture = (struct fixture){ .fail_index = 0 };
	resources[0] = resources[1] = NULL;
	assert(mister_magik_reserve_scanout_slots(starts, map, resources, 2,
		request_slot, release_slot, &fixture) == -EBUSY);
	assert(fixture.released == 0 && !resources[0] && !resources[1]);
	fixture = (struct fixture){ .fail_index = 1 };
	resources[0] = resources[1] = NULL;
	assert(mister_magik_reserve_scanout_slots(starts, map, resources, 2,
		request_slot, release_slot, &fixture) == -EBUSY);
	assert(fixture.released == 1 && !resources[0] && !resources[1]);
	return 0;
}
