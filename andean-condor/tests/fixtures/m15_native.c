/* Stateful ABI markers, NOT CVVDP math. The pinned headers check signatures. */
#define VSHIP_STATIC
#include "../../../av-metrics-fmetrics/tests/headers/fmetrics.h"
#include "../../../av-metrics-vship/tests/headers/VshipAPI.h"
#include <stdio.h>
#include <string.h>

typedef struct { int history; double sum; } State;
static void event(const char *name) {
    FILE *f = fopen(getenv("M15_LOG"), "a");
    if (!f) abort();
    fprintf(f, "%s\n", name);
    fclose(f);
}
static int fail(const char *name) {
    const char *s = getenv("M15_FAIL");
    return s && !strcmp(s, name);
}
static double marker(State *s, int sample) {
    double value = sample + 1000 * s->history + 10 * s->sum;
    s->history = sample;
    s->sum += sample;
    return value;
}
const char *fmetrics_version_str(void) { return "0.1.0"; }
const char *fmetrics_cvvdp_version_str(void) { return "M15-boundary-not-CVVDP"; }
const char *fmetrics_error_str(FmetricsErr e) { (void)e; return "M15 combined reset failure"; }
FmetricsWorkspace *fmetrics_workspace_create(void) { return (FmetricsWorkspace *)calloc(1, sizeof(State)); }
void fmetrics_workspace_destroy(FmetricsWorkspace *p) { free(p); }
FmetricsErr fmetrics_ssimu2_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) {
    (void)w; (void)a; (void)b; *s = 42; return FMETRICS_OK;
}
FmetricsErr fmetrics_iwssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_msssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_ssimu2_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s, uint32_t *m) { (void)m; return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_butteraugli_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s) { (void)o; return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_butteraugli_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s, uint32_t *m) { (void)m; return fmetrics_butteraugli_cmp(w,a,b,o,s); }
FmetricsErr fmetrics_cvvdp_create(int w, int h, float fps, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpCtx **out) {
    (void)w; (void)h; (void)fps; (void)m; (void)t; (void)p;
    event("fc+"); *out = (FmetricsCvvdpCtx *)calloc(1, sizeof(State)); return FMETRICS_OK;
}
void fmetrics_cvvdp_destroy(FmetricsCvvdpCtx *p) { event("fc-"); free(p); }
FmetricsErr fmetrics_cvvdp_process_frame(FmetricsCvvdpCtx *c, const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpResult *r) {
    (void)b; event("fp"); r->jod = marker((State *)c, *(const uint8_t *)a->data); r->quality = r->jod + 0.5; return FMETRICS_OK;
}
FmetricsErr fmetrics_cvvdp_reset(FmetricsCvvdpCtx *c) {
    event("fr"); if (fail("fr")) return FMETRICS_ERR_INTERNAL;
    memset(c, 0, sizeof(State)); return FMETRICS_OK;
}
FmetricsErr fmetrics_cvvdp_cmp(const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpResult *r) { (void)a; (void)b; (void)m; (void)t; (void)p; (void)r; return FMETRICS_OK; }
FmetricsErr fmetrics_cvvdp_get_display_params(FmetricsCvvdpDisplayModel m, FmetricsCvvdpDisplayParams *p) { (void)m; (void)p; return FMETRICS_OK; }

Vship_Version Vship_GetVersion(void) { return (Vship_Version){5,1,2,Vship_CPU}; }
Vship_Exception Vship_GetDeviceCount(int *n) { *n=1; return Vship_NoError; }
Vship_Exception Vship_GetDeviceInfo(Vship_DeviceInfo *info, int gpu) { (void)gpu; memset(info,0,sizeof(*info)); return Vship_NoError; }
Vship_Exception Vship_GPUFullCheck(int gpu) { (void)gpu; return Vship_NoError; }
int Vship_GetErrorMessage(Vship_Exception e, char *out, int len) { (void)e; return snprintf(out,(size_t)len,"M15 reset failure"); }
int Vship_GetDetailedLastError(char *out, int len) { (void)out; (void)len; return 0; }
int Vship_GetDetailedLastErrorHandler(Vship_Handle h, char *out, int len) { (void)h; (void)out; (void)len; return 0; }
Vship_Exception Vship_InitHandler(Vship_Handle *h, void *arg) {
    if (*(Vship_StructType *)arg != Vship_StructInitCVVDP_1) abort();
    event("vc+"); *h=calloc(1,sizeof(State)); return Vship_NoError;
}
Vship_Exception Vship_FreeHandler(Vship_Handle h) { event("vc-"); free(h); return Vship_NoError; }
Vship_Exception Vship_ComputeHandler(Vship_Handle h, void *out, const uint8_t *a[3], const uint8_t *b[3], const int64_t as[3], const int64_t bs[3]) {
    (void)b; (void)as; (void)bs; event("vp");
    Vship_ScoreCVVDP *s=out; s->structType=Vship_StructScoreCVVDP; s->score=marker(h,*a[0]); return Vship_NoError;
}
Vship_Exception Vship_Reset(Vship_Handle h) {
    event("vr"); if (fail("vr")) return (Vship_Exception)1;
    ((State *)h)->history=0; return Vship_NoError;
}
Vship_Exception Vship_ResetScore(Vship_Handle h) {
    event("vs"); if (fail("vs")) return (Vship_Exception)1;
    ((State *)h)->sum=0; return Vship_NoError;
}
Vship_Exception Vship_PinnedMalloc2(void **p, uint64_t size, int gpu) { (void)gpu; *p=malloc((size_t)size); return Vship_NoError; }
Vship_Exception Vship_PinnedFree2(void *p, int gpu) { (void)gpu; free(p); return Vship_NoError; }
