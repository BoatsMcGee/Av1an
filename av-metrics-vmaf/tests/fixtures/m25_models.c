/* Pinned safe picture sink; model markers test selection, not VMAF accuracy. */
#define vmaf_version m09_version
#define vmaf_init m09_init
#define vmaf_close m09_close
#define vmaf_model_load m09_model_load
#define vmaf_model_load_from_path m09_model_load_from_path
#define vmaf_model_destroy m09_model_destroy
#include "m09_ownership.c"
#undef vmaf_version
#undef vmaf_init
#undef vmaf_close
#undef vmaf_model_load
#undef vmaf_model_load_from_path
#undef vmaf_model_destroy

/* version, init, builtin, path, model allocation, model destroy, close, marker */
static unsigned calls[8];
static char selected_path[4096];
unsigned m25_count(unsigned index) { return calls[index]; }
const char *m25_path(void) { return selected_path; }
const char *vmaf_version(void) { calls[0]++; return "3.2.0"; }
int vmaf_init(Context **out, Configuration cfg) {
    calls[1]++;
    return m09_init(out, cfg);
}
int vmaf_close(Context *ctx) { calls[6]++; return m09_close(ctx); }
static int allocate_model(void **out) {
    *out = malloc(1);
    if (!*out) return -12;
    calls[4]++;
    return 0;
}
int vmaf_model_load(void **out, void *cfg, const char *name) {
    (void)cfg; (void)name;
    calls[2]++;
    if (setting("M25_NO_BUILTIN")) return -22;
    return allocate_model(out);
}
int vmaf_model_load_from_path(void **out, void *cfg, const char *path) {
    (void)cfg;
    calls[3]++;
    if (snprintf(selected_path, sizeof(selected_path), "%s", path) >= (int)sizeof(selected_path))
        abort();
    FILE *file = fopen(path, "r");
    if (!file) return -2;
    int marker = fgetc(file);
    fclose(file);
    calls[7] = (unsigned)marker;
    int status = allocate_model(out);
    if (status) return status;
    /* Like the pinned JSON loader, failure leaves an owned initialized partial. */
    return marker == '!' ? -87 : 0;
}
void vmaf_model_destroy(void *handle) { free(handle); calls[5]++; }
