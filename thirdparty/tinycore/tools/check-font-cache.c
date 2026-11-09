/* Test-only: load existing caches without the scan/rebuild APIs. */
typedef struct _FcConfig FcConfig;
typedef struct _FcCache FcCache;
extern FcConfig *FcInitLoadConfig(void);
extern FcCache *FcDirCacheLoad(const unsigned char *, FcConfig *, unsigned char **);
extern void FcDirCacheUnload(FcCache *);
extern void FcConfigDestroy(FcConfig *);
extern int FcCacheNumFont(const FcCache *);
extern int FcCacheNumSubdir(const FcCache *);
extern int puts(const char *);

int main(void) {
    static const char *dirs[] = {
        "/usr/local/lib/X11/fonts", "/usr/local/lib/X11/fonts/75dpi",
        "/usr/local/lib/X11/fonts/TTF", "/usr/local/lib/X11/fonts/misc"
    };
    static const int fonts[] = {0, 48, 1, 48};
    FcConfig *config = FcInitLoadConfig();
    if (!config) return 1;
    int failed = 0;
    for (unsigned i = 0; i < 4; ++i) {
        FcCache *cache = FcDirCacheLoad((const unsigned char *)dirs[i], config, 0);
        if (!cache) { puts(dirs[i]); failed = 1; }
        else {
            if (FcCacheNumFont(cache) != fonts[i]
                || FcCacheNumSubdir(cache) != (i == 0 ? 3 : 0)) failed = 1;
            FcDirCacheUnload(cache);
        }
    }
    FcConfigDestroy(config);
    if (!failed) puts("FONT-CACHE-LOAD-PASS directories=4");
    return failed;
}
