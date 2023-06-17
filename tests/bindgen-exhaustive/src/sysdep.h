#if defined __has_include
	#if __has_include(<linux/netfilter.h>)
		#define HAS_LINUX_HEADER
	#endif
	#if __has_include(<winsock2.h>)
		#define HAS_WINDOWS_HEADER
	#endif
	#if __has_include(<mach/mach_time.h>)
		#define HAS_MACOS_HEADER
	#endif
#endif

#ifdef __linux__
	struct zigbuild_is_linux { int x; };
	#if defined __has_include && !defined HAS_LINUX_HEADER
		#error "linux targets are expected to have <linux/netfilter.h>"
	#endif
#else
	#if defined __has_include && defined HAS_LINUX_HEADER
		#error "non-linux targets mistakenly have <linux/netfilter.h>, probably from host includes"
	#endif
#endif

#ifdef _WIN32
	struct zigbuild_is_win32 { int x; };
	#if defined __has_include && !defined HAS_WINDOWS_HEADER
		#error "windows targets are expected to have <winsock2.h>"
	#endif
#else
	#if defined __has_include && defined HAS_WINDOWS_HEADER
		#error "non-windows targets mistakenly have <winsock2.h>, probably from host includes"
	#endif
#endif

#if defined __APPLE__ && defined __MACH__
	struct zigbuild_is_macos { int x; };
	#if defined __has_include && !defined HAS_MACOS_HEADER
		#error "macos targets are expected to have <mach/mach_time.h>"
	#endif
#else
	#if defined __has_include && defined HAS_MACOS_HEADER
		#error "non-macos targets mistakenly have <mach/mach_time.h>, probably from host includes"
	#endif
#endif
