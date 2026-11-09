/* No enlarged picture writes: rejection is observed through counted calls. */
#define vmaf_version m09_version
#define vmaf_init m09_init
#define vmaf_preallocate_pictures m09_preallocate
#define vmaf_fetch_preallocated_picture m09_fetch
#include "m09_ownership.c"
#undef vmaf_version
#undef vmaf_init
#undef vmaf_preallocate_pictures
#undef vmaf_fetch_preallocated_picture

static unsigned calls[3];
unsigned m13_count(unsigned index) { return calls[index]; }
const char *vmaf_version(void) {
    const char *version = getenv("M13_VERSION");
    if (!strcmp(version, "null")) return NULL;
    if (!strcmp(version, "non-UTF8")) return "\xff";
    return version;
}
int vmaf_init(Context **out, Configuration cfg) {
    calls[0]++;
    return m09_init(out, cfg);
}
int vmaf_preallocate_pictures(Context *ctx, PoolConfiguration cfg) {
    calls[1]++;
    return m09_preallocate(ctx, cfg);
}
int vmaf_fetch_preallocated_picture(Context *ctx, Picture *out) {
    calls[2]++;
    return m09_fetch(ctx, out);
}
/* Presence only: the loader must NEVER call these unverified entry points. */
#if M13_MARKER == 1
void vmaf_picture_convert_context_init(void) { abort(); }
#elif M13_MARKER == 2
void vmaf_picture_convert(void) { abort(); }
#elif M13_MARKER == 3
void vmaf_picture_convert_context_close(void) { abort(); }
#elif M13_MARKER == 4
void vmaf_picture_wrap(void) { abort(); }
#endif
