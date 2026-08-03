#!/usr/bin/env python3
"""Whole-bundle integrity gate for the Archive pilot.

Replaces the shell implementation, which could not be made correct.

`codesign --verify --deep --strict` does not treat `Contents/Resources` as
nested code, so an unsigned Mach-O parked there ships inside the signed,
notarized bundle. Enumerating the whole tree closes that, but doing it in
shell introduced two defects of its own:

- The rejection loop read `xattr(1)` output line by line, so one attribute
  whose *name* contains a newline printed as two lines. If both halves were
  allowlisted the attribute passed, and `xattr -p` could then retrieve
  neither phantom name, so its bytes never entered the digest -- unbounded
  smuggled content with a byte-identical `tree_sha256`.
- Widening the walk to every entry fed FIFOs to `xattr`, which opens the
  path; opening a FIFO with no writer blocks forever. Candidate code could
  plant one and stall the post-execution re-gate for the whole job timeout.

`os.listxattr` returns a real list of names, so a newline in a name is just a
character, and it does not open the file, so a FIFO cannot block it. Bundles
are additionally required to contain only regular files and directories,
which removes the FIFO carrier at the source.

Usage:
  archive-bundle-integrity.py <app> [executable] [identifier]
"""

from __future__ import annotations

import ctypes
import ctypes.util
import hashlib
import os
import plistlib
import stat
import sys

# Attributes macOS applies itself. Everything else is refused.
ALLOWED_XATTRS = frozenset(
    {b"com.apple.provenance", b"com.apple.quarantine", b"com.apple.macl"}
)
# Even an allowlisted name is a shipping channel: `com.apple.macl` carrying
# 8 KB survives ditto and codesign. Real values are tiny.
MAX_XATTR_BYTES = 1024
# Aggregate ceiling across the whole bundle.
MAX_TOTAL_XATTR_BYTES = 64 * 1024


# `os.listxattr`/`os.getxattr` are Linux-only, so call libc directly. The
# kernel returns names NUL-separated, which is what makes this newline-safe:
# an attribute name containing a newline is just bytes in one entry, never
# two lines to be allowlisted independently as it was under `xattr(1)`.
_LIBC = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
_XATTR_NOFOLLOW = 0x0001
_ACL_TYPE_EXTENDED = 0x00000100

# Both return ssize_t; leaving restype at the default c_int truncates on
# 64-bit and silently becomes wrong.
_LIBC.listxattr.restype = ctypes.c_ssize_t
_LIBC.listxattr.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
_LIBC.getxattr.restype = ctypes.c_ssize_t
_LIBC.getxattr.argtypes = [
    ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p,
    ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int,
]
_LIBC.acl_get_link_np.restype = ctypes.c_void_p
_LIBC.acl_get_link_np.argtypes = [ctypes.c_char_p, ctypes.c_uint]
_LIBC.acl_to_text.restype = ctypes.c_char_p
_LIBC.acl_to_text.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ssize_t)]
_LIBC.acl_free.restype = ctypes.c_int
_LIBC.acl_free.argtypes = [ctypes.c_void_p]


def acl_text(path: str) -> str:
    """The entry's extended ACL, or "" when it carries none.

    Read through libc rather than by parsing `ls -leR`. That parse missed
    every dot-prefixed entry (`ls` needs `-a`, and `-R` does not descend into
    dot directories) and the bundle root itself (`ls -leR <dir>` lists the
    directory's contents, never the directory), so a world-writable ACL on
    `Contents/Resources/.config` shipped with a byte-identical digest. It also
    ran the other way: a filename containing a newline printed as two lines
    and forged an ACE, rejecting a clean bundle. Parsing a tool's stdout is
    the same mistake the xattr path was rewritten to avoid.
    """
    acl = _LIBC.acl_get_link_np(os.fsencode(path), _ACL_TYPE_EXTENDED)
    if not acl:
        return ""
    try:
        text = _LIBC.acl_to_text(acl, None)
        return text.decode(errors="replace") if text else ""
    finally:
        _LIBC.acl_free(acl)


def list_xattrs(path: str) -> list[bytes]:
    encoded = os.fsencode(path)
    size = _LIBC.listxattr(encoded, None, 0, _XATTR_NOFOLLOW)
    if size < 0:
        raise OSError(ctypes.get_errno(), "listxattr", path)
    if size == 0:
        return []
    buffer = ctypes.create_string_buffer(size)
    size = _LIBC.listxattr(encoded, buffer, size, _XATTR_NOFOLLOW)
    if size < 0:
        raise OSError(ctypes.get_errno(), "listxattr", path)
    return [name for name in buffer.raw[:size].split(b"\x00") if name]


def get_xattr(path: str, name: bytes) -> bytes:
    encoded = os.fsencode(path)
    size = _LIBC.getxattr(encoded, name, None, 0, 0, _XATTR_NOFOLLOW)
    if size < 0:
        raise OSError(ctypes.get_errno(), "getxattr", path)
    if size == 0:
        return b""
    buffer = ctypes.create_string_buffer(size)
    size = _LIBC.getxattr(encoded, name, buffer, size, 0, _XATTR_NOFOLLOW)
    if size < 0:
        raise OSError(ctypes.get_errno(), "getxattr", path)
    return buffer.raw[:size]


def reject(message: str) -> None:
    print(f"REJECT: {message}", file=sys.stderr)
    raise SystemExit(1)


def is_mach_o(path: str) -> bool:
    with open(path, "rb") as handle:
        magic = handle.read(4)
    return magic in {
        b"\xcf\xfa\xed\xfe",  # 64-bit little endian
        b"\xce\xfa\xed\xfe",  # 32-bit little endian
        b"\xfe\xed\xfa\xcf",  # 64-bit big endian
        b"\xfe\xed\xfa\xce",  # 32-bit big endian
        b"\xca\xfe\xba\xbe",  # universal
        b"\xbe\xba\xfe\xca",  # universal, swapped
        # 64-bit universal. Omitting these let a real `lipo -create -fat64`
        # binary carrying two runnable slices sit in Contents/Resources,
        # pass every gate and the digest binding, survive ditto and
        # codesign --deep --strict, and ship inside the notarized bundle.
        b"\xca\xfe\xba\xbf",  # universal 64
        b"\xbf\xba\xfe\xca",  # universal 64, swapped
    }


def walk(app: str) -> list[str]:
    """Every entry under `app`, plus `app` itself, sorted byte-wise."""
    entries = [app]

    def on_error(error: OSError) -> None:
        reject(f"cannot enumerate {error.filename!r}: {error}")

    for root, directory_names, file_names in os.walk(app, onerror=on_error):
        for name in directory_names + file_names:
            entries.append(os.path.join(root, name))
    return sorted(entries, key=os.fsencode)


def main() -> None:
    app = os.path.normpath(sys.argv[1])
    expected_executable = sys.argv[2] if len(sys.argv) > 2 else "minutes-archive-app"
    expected_identifier = sys.argv[3] if len(sys.argv) > 3 else "com.useminutes.archive"

    if not os.path.isdir(app):
        reject(f"{app} is not a directory")

    entries = walk(app)

    # Only regular files and directories may ship. This refuses symlinks,
    # FIFOs, sockets and devices in one rule, and removes the carrier that
    # made the previous gate hang.
    mach_o: list[str] = []
    for entry in entries:
        mode = os.lstat(entry).st_mode
        if stat.S_ISLNK(mode):
            reject(f"bundle contains a symbolic link: {entry[len(app):]}")
        if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
            reject(
                f"bundle contains a non-regular entry: {entry[len(app):]} "
                f"(mode {stat.S_IFMT(mode):#o})"
            )
        # AppleDouble carriers are silently consumed by `ditto -x -k`, so a
        # bundle containing them is not the bundle that ships and the digest
        # taken before packaging cannot be compared with the one taken after.
        if os.path.basename(entry).startswith("._"):
            reject(f"bundle contains an AppleDouble entry: {entry[len(app):]}")
        # An ACL is rejected because it "would let the delivered app ship a
        # world-writable file". Plain mode bits do exactly the same, and a
        # content-only digest cannot see them, so a permission change across
        # packaging was undetectable. A world-writable resource in an
        # installed app is a local content-substitution primitive.
        if mode & (stat.S_IWGRP | stat.S_IWOTH):
            reject(f"entry {entry[len(app):]!r} is group- or world-writable ({mode & 0o7777:#o})")
        if mode & (stat.S_ISUID | stat.S_ISGID):
            reject(f"entry {entry[len(app):]!r} is setuid or setgid ({mode & 0o7777:#o})")
        if stat.S_ISREG(mode) and is_mach_o(entry):
            mach_o.append(entry)

    if len(mach_o) != 1:
        for found in mach_o:
            print(f"  {found}", file=sys.stderr)
        reject(f"bundle must contain exactly one Mach-O; found {len(mach_o)}")

    with open(os.path.join(app, "Contents", "Info.plist"), "rb") as handle:
        info = plistlib.load(handle)
    identifier = info.get("CFBundleIdentifier")
    if identifier != expected_identifier:
        reject(f"CFBundleIdentifier is {identifier!r}, expected {expected_identifier!r}")
    executable = info.get("CFBundleExecutable")
    if executable != expected_executable:
        reject(f"CFBundleExecutable is {executable!r}, expected {expected_executable!r}")

    entry_point = os.path.join(app, "Contents", "MacOS", executable)
    if mach_o[0] != entry_point:
        reject(
            "the only Mach-O is not the declared entry point\n"
            f"  found: {mach_o[0]}\n  entry: {entry_point}"
        )
    if not os.access(entry_point, os.X_OK):
        reject("the declared entry point is not executable")

    # Digest: one NUL-terminated, \037-separated record per entry and per
    # extended attribute. Paths are used verbatim and content is hashed from
    # bytes, so no filename ever passes through a shell or a hash tool's
    # output escaping.
    digest = hashlib.sha256()
    total_xattr_bytes = 0
    for entry in entries:
        relative = entry[len(app):]
        if os.path.isdir(entry):
            kind, payload = "d", ""
        else:
            file_hash = hashlib.sha256()
            with open(entry, "rb") as handle:
                for chunk in iter(lambda: handle.read(1 << 20), b""):
                    file_hash.update(chunk)
            kind, payload = "f", file_hash.hexdigest()
        digest.update(
            f"{kind}\037{relative}\037{os.lstat(entry).st_mode & 0o7777:o}\037{payload}\000".encode()
        )

        # ACLs are invisible to mode bits and to extended attributes, survive
        # ditto, and would let the delivered app ship a world-writable file.
        acl = acl_text(entry)
        if acl:
            reject(
                f"entry {relative!r} carries an access control entry: "
                + acl.strip().replace("\n", "; ")
            )

        for attribute in sorted(list_xattrs(entry)):
            # Checked before the value is materialised: a disallowed 4 MB
            # attribute was fully read into memory only to be rejected.
            if attribute not in ALLOWED_XATTRS:
                reject(f"unexpected extended attribute {attribute!r} on {relative!r}")
            value = get_xattr(entry, attribute)
            if len(value) > MAX_XATTR_BYTES:
                reject(
                    f"extended attribute {attribute!r} on {relative!r} carries "
                    f"{len(value)} bytes, over the {MAX_XATTR_BYTES} byte limit"
                )
            # The per-attribute cap alone is not a budget: three allowlisted
            # names at 1 KB across a few hundred resources smuggled 614 KB.
            total_xattr_bytes += len(value)
            if total_xattr_bytes > MAX_TOTAL_XATTR_BYTES:
                reject(
                    f"extended attributes carry more than "
                    f"{MAX_TOTAL_XATTR_BYTES} bytes across the bundle"
                )
            digest.update(
                b"x\037"
                + os.fsencode(relative)
                + b"\037"
                + attribute
                + b"\037"
                + hashlib.sha256(value).hexdigest().encode()
                + b"\000"
            )

    print(f"mach_o_count={len(mach_o)}")
    print(f"identifier={identifier}")
    print(f"entry={executable}")
    print(f"tree_sha256={digest.hexdigest()}")


if __name__ == "__main__":
    try:
        main()
    except OSError as error:
        # Fail closed with a gate message rather than an uncaught traceback.
        reject(f"{error}")
