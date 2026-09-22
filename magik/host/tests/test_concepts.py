from magik.concepts import validate
import pytest

def sample():
    return {"sha256":"abc", "window":dict(instrumented=False,start_ms=2000,end_ms=32000,elapsed_ms=30000,context={"concept":"diagnostic","preset":"default","route":"hdmi"},process_cpu_percent=75,peak_rss_bytes=4_000_000,refresh_hz=60,drop_baseline_available=True,presentations=1800,physical_latch_posts=1800,physical_latch_flips=1800,presented_vblanks=1800,owned_vblanks=1800,physical_drops=0,latch_drops=0,latch_rejections=0)}

def test_valid_window():
    assert validate(sample(),"abc","diagnostic","default")["qualified"]

@pytest.mark.parametrize("key,value",[("physical_drops",1),("latch_drops",1),("process_cpu_percent",150),("peak_rss_bytes",134217729),("presented_vblanks",1799)])
def test_failed_gates_retained(key,value):
    data=sample();data["window"][key]=value
    assert not validate(data,"abc","diagnostic","default")["qualified"]

def test_unknown_evidence_is_not_zero():
    data=sample();data["window"]["process_cpu_percent"]=None
    with pytest.raises(ValueError):validate(data,"abc","diagnostic","default")

def test_identity_mismatch():
    with pytest.raises(ValueError):validate(sample(),"wrong","diagnostic","default")
