# Menu FPGA Warning Waiver Ledger

The stock and patched builds must use the same pinned Menu commit, Quartus
17.0.0.595, canonical seed 2, project settings, and source tree. The automated
delta gate requires their normalized warning identity multisets to match
exactly. This is a waiver for inherited upstream warnings only; it is not
permission to add a warning in the MagiK delta.

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
