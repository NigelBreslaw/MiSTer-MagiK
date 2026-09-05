from magik2.token_store import TokenStore


def test_token_store_is_device_scoped_and_mode_restricted(tmp_path) -> None:
    store = TokenStore(tmp_path, "mister.local")
    assert store.load() is None
    store.save("native-token")
    assert store.load() == "native-token"
    assert store.path.stat().st_mode & 0o777 == 0o600
    assert TokenStore(tmp_path, "other-mister").load() is None
