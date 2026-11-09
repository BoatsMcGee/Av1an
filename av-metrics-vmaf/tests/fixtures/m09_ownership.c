/* Behavioral libvmaf ABI double, not CUDA or a real thread pool.
 * Tombstones survive until close: broken Rust ownership reports counters,
 * never a real use-after-free, double free, or blocked pool shutdown. */
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
typedef struct Record {
    Picture picture;
    unsigned refs, rust_owner, worker;
    struct Record *next;
} Record;
typedef struct {
    Configuration cfg;
    Geometry geometry;
    Record *records;
    unsigned next_index;
} Context;

enum {
    FETCH, CHECKOUT, READ, TRANSFER, RUST_RETURN, WORKER_REF, WORKER_RELEASE,
    NATIVE_RELEASE, DUPLICATE, ZERO_UNREF, CLOSE_OWNED, CLOSE, CLOSE_REFS,
    BAD_INDEX, MODEL_DESTROY, RUST_UNREF, ALLOC, FREE, COUNTERS
};
static unsigned counts[COUNTERS];
static int model;
unsigned m09_count(unsigned index) { return counts[index]; }
static unsigned setting(const char *name) {
    const char *value = getenv(name);
    return value ? (unsigned)atoi(value) : 0;
}
const char *vmaf_version(void) { return "3.2.0"; }
int vmaf_init(Context **out, Configuration cfg) {
    memset(counts, 0, sizeof(counts));
    *out = calloc(1, sizeof(Context));
    if (!*out) return -12;
    counts[ALLOC]++;
    (*out)->cfg = cfg;
    return 0;
}
int vmaf_preallocate_pictures(Context *ctx, PoolConfiguration cfg) {
    ctx->geometry = cfg.pic_params;
    return 0;
}
int vmaf_fetch_preallocated_picture(Context *ctx, Picture *out) {
    counts[FETCH]++;
    Record *r = calloc(1, sizeof(*r));
    if (!r) return -12;
    counts[ALLOC]++;
    r->next = ctx->records;
    ctx->records = r;
    Picture *p = &r->picture;
    p->pix_fmt = ctx->geometry.pix_fmt;
    p->bpc = ctx->geometry.bpc;
    for (unsigned i = 0; i < 3; i++) {
        p->w[i] = ctx->geometry.w >> (i != 0);
        p->h[i] = ctx->geometry.h >> (i != 0);
        p->stride[i] = p->w[i] * (p->bpc > 8 ? 2 : 1);
        p->data[i] = calloc(p->h[i], p->stride[i]);
        if (!p->data[i]) return -12;
        counts[ALLOC]++;
    }
    p->ref = p->priv = r;
    *out = *p;
    /* Native pool failure rolls back internally but can leave populated fields.
     * This tombstone is deliberately NOT a checkout or a caller-owned ref. */
    if (counts[FETCH] == setting("M09_FAIL_FETCH")) return -72;
    r->refs = r->rust_owner = 1;
    counts[CHECKOUT]++;
    return 0;
}
int vmaf_picture_unref(Picture *p) {
    counts[RUST_UNREF]++;
    if (!p || !p->ref) {
        counts[ZERO_UNREF]++;
        return -22;
    }
    Record *r = p->ref;
    if (!r->rust_owner) {
        counts[DUPLICATE]++;
        return -22;
    }
    r->rust_owner = 0;
    r->refs--;
    counts[RUST_RETURN]++;
    memset(p, 0, sizeof(*p));
    return 0;
}
static void release_workers(Context *ctx) {
    for (Record *r = ctx->records; r; r = r->next) {
        if (r->worker) {
            r->worker = 0;
            r->refs--;
            counts[WORKER_RELEASE]++;
        }
    }
}
int vmaf_read_pictures(Context *ctx, Picture *ref, Picture *dist, unsigned index) {
    if (!ref && !dist) {
        release_workers(ctx);
        return setting("M09_FAIL_FLUSH") ? -73 : 0;
    }
    counts[READ]++;
    if (index != ctx->next_index) counts[BAD_INDEX]++;
    Picture *pair[] = { ref, dist };
    for (unsigned i = 0; i < 2; i++) {
        Record *r = pair[i]->ref;
        if (!r || !r->rust_owner) { counts[DUPLICATE]++; return -22; }
        if (ctx->cfg.n_threads) {
            r->refs++;
            r->worker = 1;
            counts[WORKER_REF]++;
        }
    }
    /* Enqueue-like rejection cleans worker copies but keeps caller ownership. */
    if (setting("M09_REJECT_READ")) {
        release_workers(ctx);
        return -71;
    }
    for (unsigned i = 0; i < 2; i++) {
        Record *r = pair[i]->ref;
        r->rust_owner = 0;
        r->refs--;
        counts[TRANSFER]++;
        counts[NATIVE_RELEASE]++;
        if (!setting("M09_NONZERO")) memset(pair[i], 0, sizeof(Picture));
    }
    ctx->next_index++;
    return 0;
}
int vmaf_close(Context *ctx) {
    counts[CLOSE]++;
    release_workers(ctx);
    for (Record *r = ctx->records; r;) {
        counts[CLOSE_OWNED] += r->rust_owner;
        counts[CLOSE_REFS] += r->refs;
        Record *next = r->next;
        for (unsigned i = 0; i < 3; i++) {
            if (r->picture.data[i]) {
                free(r->picture.data[i]);
                counts[FREE]++;
            }
        }
        free(r);
        counts[FREE]++;
        r = next;
    }
    free(ctx);
    counts[FREE]++;
    return 0;
}
int vmaf_model_load(void **out, void *cfg, const char *name) {
    (void)cfg; (void)name;
    *out = &model;
    return 0;
}
int vmaf_model_load_from_path(void **out, void *cfg, const char *path) {
    return vmaf_model_load(out, cfg, path);
}
void vmaf_model_destroy(void *handle) { (void)handle; counts[MODEL_DESTROY]++; }
int vmaf_use_features_from_model(Context *ctx, void *handle) {
    (void)ctx; (void)handle; return 0;
}
int vmaf_use_feature(Context *ctx, const char *name, void *opts) {
    (void)ctx; (void)name; (void)opts; return 0;
}
int vmaf_score_at_index(Context *ctx, void *handle, double *out, unsigned index) {
    (void)handle;
    if (index >= ctx->next_index) return -22;
    *out = 99.0;
    return 0;
}
int vmaf_feature_score_pooled(Context *ctx, const char *name, int method,
                              double *out, unsigned low, unsigned high) {
    (void)ctx; (void)name; (void)method; (void)low; (void)high;
    *out = 1.0;
    return 0;
}
