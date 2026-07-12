/* SPDX-License-Identifier: GPL-2.0 */
#ifndef MISTER_MAGIK_SCANOUT_POLICY_H
#define MISTER_MAGIK_SCANOUT_POLICY_H

typedef void *(*mister_magik_request_slot_fn)(unsigned long start,
					      unsigned long size,
					      unsigned int index,
					      void *context);
typedef void (*mister_magik_release_slot_fn)(unsigned long start,
					     unsigned long size,
					     void *resource,
					     unsigned int index,
					     void *context);

static inline bool mister_magik_scanout_ranges_valid(unsigned long visible_start,
						     unsigned long visible_size,
						     unsigned long slot0,
						     unsigned long slot1,
						     unsigned long map_size,
						     unsigned long address_max)
{
	unsigned long visible_end;
	unsigned long slot0_end;
	unsigned long slot1_end;

	if (!visible_size || !map_size || visible_start > address_max ||
	    slot0 > address_max || slot1 > address_max)
		return false;
	if (visible_size - 1 > address_max - visible_start ||
	    map_size - 1 > address_max - slot0 ||
	    map_size - 1 > address_max - slot1)
		return false;
	visible_end = visible_start + visible_size - 1;
	slot0_end = slot0 + map_size - 1;
	slot1_end = slot1 + map_size - 1;
	return visible_end < slot0 && slot0_end < slot1 && slot1_end <= address_max;
}

static inline int mister_magik_reserve_scanout_slots(const unsigned long *starts,
						     unsigned long size,
						     void **resources,
						     unsigned int count,
						     mister_magik_request_slot_fn request,
						     mister_magik_release_slot_fn release,
						     void *context)
{
	unsigned int i;

	for (i = 0; i < count; i++) {
		resources[i] = request(starts[i], size, i, context);
		if (!resources[i]) {
			while (i > 0) {
				i--;
				release(starts[i], size, resources[i], i, context);
				resources[i] = NULL;
			}
			return -EBUSY;
		}
	}
	return 0;
}

#endif
