# Release-device sequential campaign baseline

The first sequential campaign baseline was measured on the committed
production UI scope at revision `780e278bf`. The typed Apple Container runner
used the `release-device` profile (`opt-level=3`, fat LTO, two codegen units,
`ui,profile` features) and a persistent external target directory.

The campaign report is `/private/tmp/mister-magik-campaign/production-campaign.json`
and the candidate report is `/private/tmp/mister-magik-campaign/production-candidate.json`.
The edit median is the campaign decision metric; a later candidate must have a
lower median and at least three of five edit samples below this baseline median
to advance the baseline.

| sample | seconds |
| --- | ---: |
| cold | 207.826 |
| no-op 1 | 3.607 |
| no-op 2 | 3.165 |
| no-op 3 | 3.172 |
| no-op 4 | 3.127 |
| no-op 5 | 3.075 |
| edit warmup | 112.653 |
| edit 1 | 92.905 |
| edit 2 | 91.672 |
| edit 3 | 90.750 |
| edit 4 | 90.558 |
| edit 5 | 95.529 |
| edit median (baseline) | **91.672** |

The cold sample includes dependency and target initialization. The no-op and
edit samples are the relevant warm-cache measurements for sequential
optimization decisions.
