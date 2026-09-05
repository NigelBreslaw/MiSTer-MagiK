import importlib.util
from pathlib import Path
import pytest

spec = importlib.util.spec_from_file_location("probe_actions", Path(__file__).resolve().parents[2] / "scenarios/actions.py")
actions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(actions)


def window():
    return {"start_ms":2000,"end_ms":7000,"elapsed_ms":5000,"width":960,"height":540,"presentations":300,"render_us_total":1000,"render_to_present_us_total":5000000,"physical_latch_posts":300,"physical_latch_flips":300,"physical_drops":0,"latch_rejections":0,"drop_baseline_available":True,"instrumented":False,"evidence_error":None}


def test_device_window_is_validated():
    assert actions.validate_window(window(), instrumented=False, seconds=5)["physical_evidence_valid"]


@pytest.mark.parametrize("key,value", [("physical_drops",1),("latch_rejections",1),("drop_baseline_available",False),("evidence_error","unavailable"),("instrumented",True),("elapsed_ms",100)])
def test_missing_or_invalid_physical_evidence_fails(key,value):
    evidence=window()
    evidence[key]=value
    with pytest.raises(AssertionError):
        actions.validate_window(evidence,instrumented=False,seconds=5)
