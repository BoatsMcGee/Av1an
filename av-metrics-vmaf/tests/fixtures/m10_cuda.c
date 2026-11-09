/* Reuse only the unrelated CPU/model exports; M09's fixture stays unchanged. */
#define vmaf_init m09_init
#define vmaf_close m09_close
#define vmaf_preallocate_pictures m09_preallocate_pictures
#include "m09_ownership.c"
#undef vmaf_init
#undef vmaf_close
#undef vmaf_preallocate_pictures

typedef struct { unsigned resource; } CudaState;
typedef struct { void *cu_ctx; } CudaConfiguration;
typedef struct { Geometry pic_params; int method; } CudaPictures;
typedef struct { CudaState state; } CudaContext;
enum {
    STATE_INIT, WRAPPER_ALLOC, WRAPPER_FREE, RELEASE_CALL, RESOURCE_RELEASE,
    CONTEXT_RELEASE, IMPORT, PREALLOCATE, CONTEXT_CLOSE, INIT, CPU_POOL,
    DUPLICATE_RELEASE, BAD_CALL, M10_COUNTERS
};
static unsigned m10_counts[M10_COUNTERS];
/* Resource tombstones outlive both descriptors, so old Rust fails safely. */
static unsigned released;
static CudaState *wrapper;
static int wrapper_freed;

unsigned m10_count(unsigned index) { return m10_counts[index]; }
int vmaf_cuda_state_init(CudaState **out, CudaConfiguration cfg) {
    (void)cfg;
    m10_counts[STATE_INIT]++;
    unsigned scenario = setting("M10_SCENARIO");
    if (scenario == 1 || scenario == 3) {
        *out = NULL;
        return scenario == 1 ? -81 : 0;
    }
    wrapper = *out = malloc(sizeof(**out));
    if (!wrapper) return -12;
    m10_counts[WRAPPER_ALLOC]++;
    /* Early native errors leave a zeroed, allocated output, not a resource. */
    wrapper->resource = scenario == 2 || scenario == 15 ? 0 : 1;
    return scenario == 2 || scenario == 8 || scenario == 15 ? -81 : 0;
}
static void release_resource(CudaState *state) {
    if (!state->resource) return;
    m10_counts[RESOURCE_RELEASE]++;
    if (released++) m10_counts[DUPLICATE_RELEASE]++;
    state->resource = 0;
}
int vmaf_cuda_release(CudaState *state) {
    m10_counts[RELEASE_CALL]++;
    release_resource(state);
    return 0;
}
void m10_free(void *pointer) {
    if (wrapper_freed || pointer != wrapper) {
        m10_counts[BAD_CALL]++;
        return;
    }
    wrapper_freed = 1;
    m10_counts[WRAPPER_FREE]++;
    wrapper = NULL;
    free(pointer);
}
int vmaf_init(CudaContext **out, Configuration cfg) {
    (void)cfg;
    m10_counts[INIT]++;
    unsigned scenario = setting("M10_SCENARIO");
    if (scenario == 4 || scenario == 5) {
        *out = NULL;
        return scenario == 4 ? -82 : 0;
    }
    *out = calloc(1, sizeof(**out));
    return *out ? 0 : -12;
}
int vmaf_cuda_import_state(CudaContext *ctx, CudaState *state) {
    m10_counts[IMPORT]++;
    if (!ctx || !state) { m10_counts[BAD_CALL]++; return -22; }
    if (setting("M10_SCENARIO") == 6) return -83;
    ctx->state = *state;
    return 0;
}
int vmaf_cuda_preallocate_pictures(CudaContext *ctx, CudaPictures cfg) {
    (void)cfg;
    m10_counts[PREALLOCATE]++;
    if (!ctx || !ctx->state.resource) m10_counts[BAD_CALL]++;
    return setting("M10_SCENARIO") == 7 ? -84 : 0;
}
int vmaf_close(CudaContext *ctx) {
    m10_counts[CONTEXT_CLOSE]++;
    if (ctx->state.resource) {
        m10_counts[CONTEXT_RELEASE]++;
        release_resource(&ctx->state);
    }
    free(ctx);
    return 0;
}
int vmaf_preallocate_pictures(CudaContext *ctx, PoolConfiguration cfg) {
    (void)ctx; (void)cfg;
    m10_counts[CPU_POOL]++;
    return 0;
}
/* Called after the snapshot even on RED; not part of the measured ownership. */
void m10_cleanup(void) {
    if (wrapper && !wrapper_freed) free(wrapper);
}
