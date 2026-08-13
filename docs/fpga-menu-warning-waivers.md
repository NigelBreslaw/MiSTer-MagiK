# Menu FPGA Warning Waiver Ledger

The stock and patched builds must use the same pinned Menu commit, Quartus
17.0.0.595, canonical seed 2, build date, project settings, and source tree.
All three matched variants use four Quartus processors with parallel synthesis
disabled. A shared preparation script applies those settings and the required
asynchronous clock-group correction in both local and GitHub builds. Cache
identity binds the preparation script, build date, processor count, and full
synthesis component identity. The automated delta gate also verifies the
reported Quartus settings and requires the normalized warning identity
multisets to match exactly. This is a waiver for inherited upstream warnings
only; it is not permission to add a warning in the MagiK delta.

The baseline build reports 50 warnings in its Quartus flow summary. Quartus
emits 24 primary warning records; continuation/detail records account for the
remaining summary count. The release artifact stores the complete stock log,
reports, and normalized warning comparison. A warning is waived only when its
code, normalized primary message, and multiplicity match that stock evidence.

The known inherited groups are:

- undriven upstream Menu output ports;
- synthesized-away and connectivity-summary notices;
- tri-state/open-drain/pin connectivity notices;
- the Quartus LogicLock subscription notice;
- ignored legacy `sys_top.sdc` filters and their false-path consequence;
- the inherited `rtl/lfsr.v` combinational-loop warning;
- ignored fast-I/O and invalid fitter assignment summaries;
- placement-effort and fitter connectivity notices.

Any new inferred latch, unconstrained endpoint, warning identity, CDC finding,
or MagiK source path is unwaived and fails release signoff. The exact ledger is
therefore the stock report set bound into each release manifest, rather than a
manually copied list that could silently drift from upstream.
