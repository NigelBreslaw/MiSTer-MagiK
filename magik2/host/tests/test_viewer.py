import socket
import threading

from magik2.viewer import WatchState


def test_closing_viewer_unblocks_a_native_read():
    local, peer = socket.socketpair()
    state = WatchState()
    state.connection = local
    finished = threading.Event()
    def read():
        try:
            local.recv(1)
        finally:
            finished.set()
    thread = threading.Thread(target=read)
    thread.start()
    try:
        state.close()
        assert finished.wait(1)
        assert state.closed.is_set()
    finally:
        peer.close()
        thread.join(timeout=1)
