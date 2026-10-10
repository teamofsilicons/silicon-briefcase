"""Native binary and Linux ABI checks shared by the release packager."""
from pathlib import Path
import struct
from check_linux_abi import verify_glibc_requirements

def verify_binary(path: Path, target: str) -> None:
    """Reject scripts, wrong operating systems and wrong CPU architectures."""
    data = path.read_bytes()
    arm = target.endswith("aarch64")
    if target.startswith("linux-"):
        valid = (
            data[:6] == b"\x7fELF\x02\x01"
            and len(data) >= 20
            and struct.unpack_from("<H", data, 18)[0] == (183 if arm else 62)
        )
    elif target.startswith("macos-"):
        valid = (
            data[:4] == b"\xcf\xfa\xed\xfe"
            and len(data) >= 8
            and struct.unpack_from("<I", data, 4)[0]
            == (0x0100000C if arm else 0x01000007)
        )
    else:
        valid = False
        if data[:2] == b"MZ" and len(data) >= 64:
            offset = struct.unpack_from("<I", data, 60)[0]
            valid = (
                len(data) >= offset + 6
                and data[offset : offset + 4] == b"PE\0\0"
                and struct.unpack_from("<H", data, offset + 4)[0]
                == (0xAA64 if arm else 0x8664)
            )
    if not valid:
        raise SystemExit(f"Wrong native binary format or architecture for {target}: {path}")

    if target.startswith("linux-"):
        try:
            verify_glibc_requirements(data)
        except ValueError as error:
            raise SystemExit(f"Unsupported Linux ABI for {target}: {path}: {error}") from error


