/* Empty stand-in for the POSIX header on Windows MSVC builds.
 *
 * rs-x11-hash 0.1.8 (dashcore's X11 hash, C code from sphlib) includes <unistd.h> in
 * src/x11/sph_types.h but uses nothing from it, and MSVC has no such header. The
 * Windows CI job puts this directory on the include path through
 * CFLAGS_x86_64_pc_windows_msvc (docs/ci.md, "Windows").
 */
