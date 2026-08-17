# Fat-LTO codegen-unit campaign results

Both candidates were measured against the production baseline median of
`91.672 s` at the same persistent Apple Container setup. Each candidate used
five edit samples after its cold/no-op/warmup sequence and required a lower
median plus at least three samples below the baseline median.

| variant | cold | no-op samples | warmup | edit samples | median | below baseline | decision |
| --- | ---: | --- | ---: | --- | ---: | ---: | --- |
| fat LTO / 4 CGUs | 228.176 s | 3.467, 2.996, 4.187, 2.896, 2.938 s | 116.751 s | 105.334, 104.706, 103.845, 103.336, 104.668 s | 104.668 s | 0/5 | loss |
| fat LTO / 8 CGUs | 197.997 s | 3.445, 2.863, 2.940, 3.587, 2.944 s | 113.205 s | 119.557, 123.584, 157.432, 121.409, 125.834 s | 123.584 s | 0/5 | loss |

The experiment commit was reverted after both candidates failed. The
canonical production profile remains fat LTO with two codegen units.
