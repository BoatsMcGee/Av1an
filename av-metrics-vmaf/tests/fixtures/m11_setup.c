/* Full-constructor ABI double, NOT a CUDA implementation or metric oracle.
 * All records/tombstones survive close until m11_cleanup, including failed
 * out-parameters. Ownership mistakes report counters, not invalid memory use. */
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    int pix_fmt;
    unsigned bpc, w[3], h[3];
    ptrdiff_t stride[3];
    void *data[3], *ref, *priv;
} Picture;
typedef struct {
    int log_level;
    unsigned n_threads, n_subsample;
    uint64_t cpumask, gpumask;
} Configuration;
typedef struct { unsigned w, h, bpc; int pix_fmt; } Geometry;
typedef struct { Geometry pic_params; unsigned pic_cnt; } PoolConfiguration;
typedef struct { Geometry pic_params; int method; } CudaPictures;
typedef struct { void *cu_ctx; } CudaConfiguration;
typedef struct { unsigned resource; } CudaState;
typedef struct Context Context;
typedef struct Model { Context *owner; unsigned destroyed; } Model;
typedef struct Record {
    Picture picture;
    Context *owner;
    unsigned owned;
    unsigned char bytes[3][64];
    struct Record *next;
} Record;
struct Context {
    unsigned cuda, closed, host_pool, device_pool, fetch, next_index, flushed;
    Configuration cfg;
    Geometry geometry;
    CudaState state;
    Model model;
    unsigned char device_canary[64];
};
enum {
    INIT, ALLOC, CLOSE, IMPORT, DEVICE_POOL, HOST_POOL, LOAD, MODEL_ALLOC,
    MODEL_DESTROY, REQUIRED, FETCH, CHECKOUT, RETURN, TRANSFER, READ, FLUSH,
    OPTIONAL, THREADS, COUNTERS
};
enum { STATE_INIT, WRAPPER_ALLOC, WRAPPER_FREE, ORIGINAL_RELEASE,
       CONTEXT_RELEASE, RESOURCE_RELEASE, BAD, GLOBAL_COUNTERS };
static unsigned counts[2][COUNTERS], globals[GLOBAL_COUNTERS];
static Context contexts[2], failed_context, *current;
static Model failed_model;
static Record *records;
static CudaState *wrapper;
static unsigned wrapper_freed, state_started, resource_live;
static unsigned setting(const char *name) {
    const char *value = getenv(name);
    return value ? (unsigned)atoi(value) : 0;
}
static unsigned stage(void) { return setting("M11_STAGE"); }
static void bad_if(int condition) { if (condition) globals[BAD]++; }
unsigned m11_count(unsigned backend, unsigned index) {
    return backend < 2 ? counts[backend][index] : globals[index];
}
const char *vmaf_version(void) { return "3.2.1"; }
int vmaf_cuda_state_init(CudaState **out, CudaConfiguration cfg) {
    bad_if(cfg.cu_ctx != NULL);
    globals[STATE_INIT]++;
    state_started = 1;
    if (stage() == 1) { *out = NULL; return -101; }
    wrapper = *out = calloc(1, sizeof(**out));
    if (!wrapper) return -12;
    globals[WRAPPER_ALLOC]++;
    wrapper->resource = resource_live = stage() != 13;
    return stage() == 13 ? -101 : 0;
}
static void release(CudaState *state) {
    if (!state->resource) return;
    bad_if(!resource_live);
    resource_live = state->resource = 0;
    globals[RESOURCE_RELEASE]++;
}
int vmaf_cuda_release(CudaState *state) {
    globals[ORIGINAL_RELEASE]++;
    release(state);
    return 0;
}
void m11_free(void *ptr) {
    bad_if(ptr != wrapper || wrapper_freed);
    if (ptr != wrapper || wrapper_freed) return;
    wrapper_freed = 1;
    globals[WRAPPER_FREE]++;
    /* Keep the wrapper tombstone until cleanup. */
}
int vmaf_init(Context **out, Configuration cfg) {
    unsigned cuda = state_started && cfg.n_threads == 0;
    counts[cuda][INIT]++;
    counts[cuda][THREADS] = cfg.n_threads;
    if (!cuda) {
        /* A CPU retry must begin AFTER all failed CUDA resources are returned. */
        bad_if(resource_live || (wrapper && !wrapper_freed));
        bad_if(counts[1][ALLOC] != counts[1][CLOSE]);
        bad_if(counts[1][MODEL_ALLOC] != counts[1][MODEL_DESTROY]);
        for (Record *r = records; r; r = r->next) bad_if(r->owned);
    }
    if (cuda && stage() == 2) {
        *out = &failed_context; /* Native init errors must NOT be closed. */
        return -102;
    }
    current = *out = &contexts[cuda];
    current->cuda = cuda;
    current->cfg = cfg;
    memset(current->device_canary, 0xa5, sizeof(current->device_canary));
    counts[cuda][ALLOC]++;
    return 0;
}
int vmaf_cuda_import_state(Context *ctx, CudaState *state) {
    counts[ctx->cuda][IMPORT]++;
    if (stage() == 3) return -103;
    ctx->state = *state;
    return 0;
}
static void geometry_check(Geometry cfg) {
    bad_if(cfg.w != 8 || cfg.h != 8 || cfg.bpc != 8 || cfg.pix_fmt != 1);
}
int vmaf_cuda_preallocate_pictures(Context *ctx, CudaPictures cfg) {
    counts[ctx->cuda][DEVICE_POOL]++;
    geometry_check(cfg.pic_params);
    bad_if(!ctx->cuda || !ctx->state.resource || cfg.method != 1);
    if (stage() == 4) return -104;
    ctx->device_pool = 1;
    return 0;
}
int vmaf_preallocate_pictures(Context *ctx, PoolConfiguration cfg) {
    counts[ctx->cuda][HOST_POOL]++;
    geometry_check(cfg.pic_params);
    bad_if(cfg.pic_cnt != (ctx->cuda ? 4 : 6));
    if (ctx->cuda && stage() == 5) return -105;
    ctx->geometry = cfg.pic_params;
    ctx->host_pool = 1;
    return 0;
}
static int load_model(void **out, int from_path) {
    unsigned cuda = current->cuda;
    counts[cuda][LOAD]++;
    if ((cuda && stage() == 6) || (stage() == 14 && !from_path)) {
        *out = &failed_model; /* Nonnull does not confer ownership on failure. */
        return -106;
    }
    if (cuda && stage() == 12) { *out = NULL; return 0; }
    bad_if(*out != NULL);
    current->model.owner = current;
    *out = &current->model;
    counts[cuda][MODEL_ALLOC]++;
    return 0;
}
int vmaf_model_load(void **out, void *cfg, const char *name) {
    (void)cfg; (void)name;
    return load_model(out, 0);
}
int vmaf_model_load_from_path(void **out, void *cfg, const char *path) {
    (void)cfg; (void)path;
    return load_model(out, 1);
}
void vmaf_model_destroy(Model *model) {
    if (!model || !model->owner) { globals[BAD]++; return; }
    bad_if(model->destroyed || model->owner->closed);
    for (Record *r = records; r; r = r->next)
        bad_if(r->owner == model->owner && r->owned);
    model->destroyed++;
    counts[model->owner->cuda][MODEL_DESTROY]++;
}
int vmaf_use_features_from_model(Context *ctx, Model *model) {
    counts[ctx->cuda][REQUIRED]++;
    bad_if(model != &ctx->model || model->owner != ctx || model->destroyed);
    if (stage() == 11) return ctx->cuda ? -107 : -207;
    return ctx->cuda && stage() == 7 ? -107 : 0;
}
int vmaf_use_feature(Context *ctx, const char *name, void *opts) {
    (void)name; (void)opts;
    counts[ctx->cuda][OPTIONAL]++;
    return stage() == 10 ? -110 : 0;
}
int vmaf_fetch_preallocated_picture(Context *ctx, Picture *out) {
    counts[ctx->cuda][FETCH]++;
    ctx->fetch++;
    if (!ctx->host_pool) return -22;
    Record *r = calloc(1, sizeof(*r));
    if (!r) return -12;
    r->next = records;
    records = r;
    r->owner = ctx;
    Picture *p = &r->picture;
    p->pix_fmt = 1;
    p->bpc = 8;
    for (unsigned i = 0; i < 3; i++) {
        p->w[i] = p->h[i] = i ? 4 : 8;
        p->stride[i] = p->w[i];
        p->data[i] = r->bytes[i]; /* Always HOST, never device_canary. */
    }
    p->ref = p->priv = r;
    *out = *p;
    if (ctx->cuda && ((stage() == 8 && ctx->fetch == 1) ||
                        (stage() == 9 && ctx->fetch == 2)))
        return stage() == 8 ? -108 : -109;
    r->owned = 1;
    counts[ctx->cuda][CHECKOUT]++;
    return 0;
}
void vmaf_picture_unref(Picture *p) {
    Record *r = p ? p->ref : NULL;
    if (!r) { globals[BAD]++; return; }
    bad_if(!r->owned || r->owner->closed);
    r->owned = 0;
    counts[r->owner->cuda][RETURN]++;
    memset(p, 0, sizeof(*p));
}
int vmaf_read_pictures(Context *ctx, Picture *ref, Picture *dist, unsigned index) {
    if (!ref && !dist) {
        counts[ctx->cuda][FLUSH]++;
        ctx->flushed = 1;
        return 0;
    }
    bad_if(index != ctx->next_index || !ctx->host_pool);
    bad_if(ctx->cuda && (!ctx->device_pool || ctx->cfg.n_threads));
    Picture *pair[] = { ref, dist };
    for (unsigned side = 0; side < 2; side++) {
        Record *r = pair[side]->ref;
        if (!r) { globals[BAD]++; return -22; }
        bad_if(r->owner != ctx || !r->owned);
        for (unsigned i = 0; i < 3; i++) {
            bad_if(pair[side]->data[i] != r->bytes[i]);
            /* Index zero exercises raw rowcopy with known source samples. */
            if (!index) for (unsigned j = 0; j < pair[side]->w[i] * pair[side]->h[i]; j++)
                bad_if(r->bytes[i][j] != (side ? 22 : 11));
        }
        r->owned = 0;
        counts[ctx->cuda][TRANSFER]++;
        /* Native success may leave caller descriptors nonzero (M09). */
    }
    ctx->next_index++;
    counts[ctx->cuda][READ]++;
    return 0;
}
int vmaf_score_at_index(Context *ctx, Model *model, double *out, unsigned index) {
    bad_if(model != &ctx->model);
    if (!ctx->flushed || index >= ctx->next_index) return -22;
    *out = ctx->cuda ? 91.0 : 81.0; /* Boundary marker, not metric equivalence. */
    return 0;
}
int vmaf_feature_score_pooled(Context *ctx, const char *name, int method,
                               double *out, unsigned low, unsigned high) {
    (void)ctx; (void)name; (void)method; (void)low; (void)high;
    *out = 1.0;
    return 0;
}
void vmaf_close(Context *ctx) {
    if (ctx == &failed_context) { globals[BAD]++; return; }
    bad_if(ctx->closed);
    bad_if(counts[ctx->cuda][MODEL_ALLOC] != counts[ctx->cuda][MODEL_DESTROY]);
    for (Record *r = records; r; r = r->next) bad_if(r->owner == ctx && r->owned);
    for (unsigned i = 0; i < sizeof(ctx->device_canary); i++)
        bad_if(ctx->device_canary[i] != 0xa5);
    if (ctx->state.resource) {
        globals[CONTEXT_RELEASE]++;
        release(&ctx->state);
    }
    ctx->closed = 1;
    counts[ctx->cuda][CLOSE]++;
}
void m11_cleanup(void) {
    while (records) { Record *next = records->next; free(records); records = next; }
    free(wrapper);
    wrapper = NULL;
}
