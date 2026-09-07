"""macOS generic passwords, through Security.framework (never a subprocess)."""

from __future__ import annotations

import ctypes as C
import sys


class KeychainError(RuntimeError):
    pass


class Keychain:
    service = b"org.mister-magik.bootstrap"

    def __init__(self) -> None:
        if sys.platform != "darwin":
            raise KeychainError("macOS Keychain is unavailable on this platform")
        try:
            self.api = C.CDLL("/System/Library/Frameworks/Security.framework/Security")
            self.core = C.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        except OSError as error:
            raise KeychainError("macOS Keychain is unavailable") from error
        self.api.SecKeychainFindGenericPassword.argtypes = [C.c_void_p, C.c_uint32, C.c_char_p, C.c_uint32, C.c_char_p, C.POINTER(C.c_uint32), C.POINTER(C.c_void_p), C.POINTER(C.c_void_p)]
        self.api.SecKeychainAddGenericPassword.argtypes = [C.c_void_p, C.c_uint32, C.c_char_p, C.c_uint32, C.c_char_p, C.c_uint32, C.c_void_p, C.POINTER(C.c_void_p)]
        self.api.SecKeychainItemModifyAttributesAndData.argtypes = [C.c_void_p, C.c_void_p, C.c_uint32, C.c_void_p]
        self.api.SecKeychainItemFreeContent.argtypes = [C.c_void_p, C.c_void_p]
        self.core.CFRelease.argtypes = [C.c_void_p]

    @staticmethod
    def _check(status: int) -> None:
        if status:
            detail = "access denied or interaction unavailable" if status in {-25293, -25308, -128} else "operation failed"
            raise KeychainError(f"macOS Keychain {detail} (status {status})")

    def _find(self, identity: str, username: str):
        account = f"{identity}/{username}".encode()
        length, data, item = C.c_uint32(), C.c_void_p(), C.c_void_p()
        status = self.api.SecKeychainFindGenericPassword(None, len(self.service), self.service, len(account), account, C.byref(length), C.byref(data), C.byref(item))
        if status == -25300:
            return account, None, None
        self._check(status)
        try:
            password = C.string_at(data, length.value).decode()
        finally:
            self.api.SecKeychainItemFreeContent(None, data)
        return account, password, item

    def load(self, identity: str, username: str) -> str | None:
        _, password, item = self._find(identity, username)
        if item:
            self.core.CFRelease(item)
        return password

    def save(self, identity: str, username: str, password: str) -> None:
        account, _, item = self._find(identity, username)
        secret = password.encode()
        try:
            if item:
                status = self.api.SecKeychainItemModifyAttributesAndData(item, None, len(secret), secret)
            else:
                status = self.api.SecKeychainAddGenericPassword(None, len(self.service), self.service, len(account), account, len(secret), secret, None)
            self._check(status)
        finally:
            if item:
                self.core.CFRelease(item)
