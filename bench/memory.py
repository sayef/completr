"""Memory a process uses: its physical footprint on macOS, which counts compressed memory, else its RSS."""
import ctypes, sys
import psutil

if sys.platform == "darwin":
    _libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
    _RUSAGE_INFO_V4 = 4
    # rusage_info_v4: a 16-byte uuid, then 35 unsigned 64-bit fields; ri_phys_footprint is the 8th and
    # ri_interval_max_phys_footprint, the peak since the last proc_reset_footprint_interval, the 34th.
    _FIELDS = 16 + 35 * 8

    def _field(pid, i):
        buf = ctypes.create_string_buffer(_FIELDS)
        if _libproc.proc_pid_rusage(ctypes.c_int(pid), _RUSAGE_INFO_V4, buf) != 0:
            return None
        return int.from_bytes(buf.raw[16 + i * 8:16 + (i + 1) * 8], "little")

    def used(pid):
        value = _field(pid, 7)
        return psutil.Process(pid).memory_info().rss if value is None else value

    def reset_peak(pid):
        """Starts a new interval for `peak`."""
        _libproc.proc_reset_footprint_interval(ctypes.c_int(pid))

    def peak(pid):
        """The kernel's exact peak footprint since `reset_peak`; None if unknown."""
        return _field(pid, 33)
else:
    def used(pid):
        return psutil.Process(pid).memory_info().rss

    def reset_peak(pid):
        pass

    def peak(pid):
        return None
