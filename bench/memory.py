"""Memory a process uses: its physical footprint on macOS, which counts compressed memory, else its RSS."""
import ctypes, sys
import psutil

if sys.platform == "darwin":
    _libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
    _RUSAGE_INFO_V2 = 2
    # rusage_info_v2: a 16-byte uuid, then 19 unsigned 64-bit fields; ri_phys_footprint is the eighth.
    _FIELDS = 16 + 19 * 8

    def used(pid):
        buf = ctypes.create_string_buffer(_FIELDS)
        if _libproc.proc_pid_rusage(ctypes.c_int(pid), _RUSAGE_INFO_V2, buf) != 0:
            return psutil.Process(pid).memory_info().rss
        return int.from_bytes(buf.raw[16 + 7 * 8:16 + 8 * 8], "little")
else:
    def used(pid):
        return psutil.Process(pid).memory_info().rss
