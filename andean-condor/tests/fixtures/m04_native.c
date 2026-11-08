/* Test-only ABI sinks. Return 42 only when the real drivers deliver the
 * independently declared metadata (and fmetrics' converted RGB samples).
 * These are not implementations of the metrics. */
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    const uint8_t *data;
    uint32_t width, height, stride;
    int format, colorspace;
    bool hdr;
} Image;

const char *fmetrics_version_str(void) { return "m04-test"; }
const char *fmetrics_cvvdp_version_str(void) { return "m04-test"; }
const char *fmetrics_error_str(int error) { return "M04 native image mismatch"; }
void *fmetrics_workspace_create(void) { return malloc(1); }
void fmetrics_workspace_destroy(void *workspace) { free(workspace); }
int fmetrics_ssimu2_cmp(void *workspace, const Image *reference, const Image *distorted, double *score) {
    const uint8_t a[] = {145, 73, 121}, b[] = {152, 75, 124};
    if (reference->colorspace != 2 || distorted->colorspace != 1 ||
        reference->format != 1 || distorted->format != 1 ||
        reference->hdr || distorted->hdr ||
        memcmp(reference->data, a, 3) || memcmp(distorted->data, b, 3)) return 2;
    *score = 42;
    return 0;
}
int fmetrics_iwssim_cmp(void *w, const Image *a, const Image *b, double *s) { return 2; }
int fmetrics_msssim_cmp(void *w, const Image *a, const Image *b, double *s) { return 2; }
int fmetrics_ssimu2_cmp_map(void *w, const Image *a, const Image *b, double *s, uint32_t *m) { return 2; }
int fmetrics_butteraugli_cmp(void *w, const Image *a, const Image *b, const void *o, double *s) { return 2; }
int fmetrics_butteraugli_cmp_map(void *w, const Image *a, const Image *b, const void *o, double *s, uint32_t *m) { return 2; }
int fmetrics_cvvdp_create(int w, int h, float fps, int model, unsigned threads, const void *params, void **ctx) { *ctx = malloc(1); return 0; }
void fmetrics_cvvdp_destroy(void *ctx) { free(ctx); }
int fmetrics_cvvdp_reset(void *ctx) { return 0; }
int fmetrics_cvvdp_process_frame(void *ctx, const Image *a, const Image *b, double *result) {
    int status = fmetrics_ssimu2_cmp(ctx, a, b, result);
    result[1] = result[0];
    return status;
}
int fmetrics_cvvdp_cmp(const Image *a, const Image *b, int model, unsigned threads, const void *params, void *result) { return 2; }
int fmetrics_cvvdp_get_display_params(int model, void *params) { return 2; }

typedef struct {
    int64_t width, height, target_width, target_height;
    int sample, range, subw, subh, chroma_location, family, matrix, transfer, primaries;
    int crop[4];
} Colorspace;
typedef struct { int type; Colorspace reference, distorted; int gpu; } Init;
typedef struct { int type; double score; } Score;
typedef struct { int major, minor, patch, backend; } Version;
Version Vship_GetVersion(void) { return (Version){5, 1, 2, 3}; }
int Vship_GetDeviceCount(int *count) { *count = 1; return 0; }
int Vship_GetDeviceInfo(void *info, int gpu) { return 0; }
int Vship_GPUFullCheck(int gpu) { return 0; }
int Vship_GetErrorMessage(int error, char *buffer, int len) { return 0; }
int Vship_GetDetailedLastError(char *buffer, int len) { return 0; }
int Vship_GetDetailedLastErrorHandler(void *handler, char *buffer, int len) { return 0; }
int Vship_InitHandler(void **handler, void *data) {
    const Init *init = data;
    const Colorspace *a = &init->reference, *b = &init->distorted;
    if (init->type != 1 || a->range != 1 || a->matrix != 6 ||
        a->transfer != 13 || a->primaries != 1 || a->chroma_location != 1 ||
        b->range != 0 || b->matrix != 9 || b->transfer != 16 ||
        b->primaries != 9 || b->chroma_location != 2) return 7;
    *handler = malloc(1);
    return 0;
}
int Vship_FreeHandler(void *handler) { free(handler); return 0; }
int Vship_ComputeHandler(void *handler, void *data, const uint8_t **a, const uint8_t **b, const int64_t *as, const int64_t *bs) {
    Score *score = data;
    score->score = 42;
    return 0;
}
int Vship_Reset(void *handler) { return 0; }
int Vship_ResetScore(void *handler) { return 0; }
