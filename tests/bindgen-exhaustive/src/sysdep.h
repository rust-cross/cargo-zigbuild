#ifdef __linux__
struct zigbuild_is_linux { int x; };
#endif

#ifdef _WIN32
struct zigbuild_is_win32 { int x; };
#endif

#if defined __APPLE__ && defined __MACH__
struct zigbuild_is_macos { int x; };
#endif
