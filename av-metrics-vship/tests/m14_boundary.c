/* ABI sink, NOT a GPU implementation. The unmodified header checks signatures. */
#define VSHIP_STATIC
#include "headers/VshipAPI.h"
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned counts[6]; /* probe, init, free, compute, reset, reset_score */
static int setting(const char *key, int fallback) { const char *s=getenv(key); return s ? atoi(s) : fallback; }
unsigned m14_count(unsigned i) { return counts[i]; }
Vship_Version Vship_GetVersion(void) { return (Vship_Version){5,1,setting("M14_PATCH",2),(Vship_Backend)setting("M14_BACKEND",3)}; }
Vship_Exception Vship_GetDeviceCount(int *n) { *n=1; return Vship_NoError; }
Vship_Exception Vship_GetDeviceInfo(Vship_DeviceInfo *info, int gpu) { (void)gpu; memset(info,0,sizeof(*info)); return Vship_NoError; }
Vship_Exception Vship_GPUFullCheck(int gpu) { (void)gpu; counts[0]++; return (Vship_Exception)setting("M14_PROBE_STATUS",0); }
int Vship_GetErrorMessage(Vship_Exception e, char *out, int len) {
    if ((unsigned)e > 17) abort();
    return snprintf(out,(size_t)len,"known vship status");
}
int Vship_GetDetailedLastError(char *out, int len) { (void)out; (void)len; return 0; }
int Vship_GetDetailedLastErrorHandler(Vship_Handle h, char *out, int len) { (void)h; (void)out; (void)len; return 0; }
Vship_Exception Vship_InitHandler(Vship_Handle *h, void *arg) { counts[1]++; *h=malloc(sizeof(int)); memcpy(*h,arg,sizeof(int)); return Vship_NoError; }
Vship_Exception Vship_FreeHandler(Vship_Handle h) { counts[2]++; free(h); return Vship_NoError; }
Vship_Exception Vship_ComputeHandler(Vship_Handle h, void *out, const uint8_t *a[3], const uint8_t *b[3], const int64_t as[3], const int64_t bs[3]) {
    (void)a; (void)b; (void)as; (void)bs; counts[3]++;
    int type=*(int*)h;
    if (type==1) ((Vship_ScoreSSIMULACRA2*)out)->score=42;
    else if (type==2) {
        Vship_ScoreButteraugli *s=out; s->normQ=42; s->norm3=43; s->norminf=44;
    } else ((Vship_ScoreCVVDP*)out)->score=42;
    *(Vship_StructType*)out=(Vship_StructType)setting("M14_TAG",type+3);
    return (Vship_Exception)setting("M14_COMPUTE_STATUS",0);
}
Vship_Exception Vship_Reset(Vship_Handle h) { (void)h; counts[4]++; return (Vship_Exception)setting("M19_RESET_STATUS",0); }
Vship_Exception Vship_ResetScore(Vship_Handle h) { (void)h; counts[5]++; return (Vship_Exception)setting("M19_RESET_STATUS",0); }
Vship_Exception Vship_PinnedMalloc2(void **p, uint64_t size, int gpu) { (void)gpu; *p=malloc((size_t)size); return Vship_NoError; }
Vship_Exception Vship_PinnedFree2(void *p, int gpu) { (void)gpu; free(p); return Vship_NoError; }

#ifdef M14_PROBE
#define TYPE(T) printf("%zu %zu ",sizeof(T),_Alignof(T))
#define FIELD(T,F) printf("%zu %zu ",offsetof(T,F),sizeof(((T*)0)->F))
int main(void) {
    TYPE(Vship_Exception); TYPE(Vship_Backend); TYPE(Vship_StructType);
    TYPE(Vship_Sample_t); TYPE(Vship_Range_t); TYPE(Vship_ChromaLocation_t); TYPE(Vship_ColorFamily_t);
    TYPE(Vship_YUVMatrix_t); TYPE(Vship_TransferFunction_t); TYPE(Vship_Primaries_t);
    TYPE(Vship_Version); FIELD(Vship_Version,major); FIELD(Vship_Version,minor); FIELD(Vship_Version,minorMinor); FIELD(Vship_Version,backend);
    TYPE(Vship_Colorspace_t); FIELD(Vship_Colorspace_t,width); FIELD(Vship_Colorspace_t,height);
    FIELD(Vship_Colorspace_t,target_width); FIELD(Vship_Colorspace_t,target_height); FIELD(Vship_Colorspace_t,sample);
    FIELD(Vship_Colorspace_t,range); FIELD(Vship_Colorspace_t,subsampling); FIELD(Vship_Colorspace_t,chromaLocation);
    FIELD(Vship_Colorspace_t,colorFamily); FIELD(Vship_Colorspace_t,YUVMatrix); FIELD(Vship_Colorspace_t,transferFunction);
    FIELD(Vship_Colorspace_t,primaries); FIELD(Vship_Colorspace_t,crop);
    TYPE(Vship_InitSSIMULACRA2_1); FIELD(Vship_InitSSIMULACRA2_1,structType); FIELD(Vship_InitSSIMULACRA2_1,src_colorspace); FIELD(Vship_InitSSIMULACRA2_1,dis_colorspace); FIELD(Vship_InitSSIMULACRA2_1,gpu_id);
    TYPE(Vship_InitButteraugli_1); FIELD(Vship_InitButteraugli_1,structType); FIELD(Vship_InitButteraugli_1,src_colorspace); FIELD(Vship_InitButteraugli_1,dis_colorspace); FIELD(Vship_InitButteraugli_1,Qnorm); FIELD(Vship_InitButteraugli_1,intensity_multiplier); FIELD(Vship_InitButteraugli_1,gpu_id);
    TYPE(Vship_InitCVVDP_1); FIELD(Vship_InitCVVDP_1,structType); FIELD(Vship_InitCVVDP_1,src_colorspace); FIELD(Vship_InitCVVDP_1,dis_colorspace); FIELD(Vship_InitCVVDP_1,fps); FIELD(Vship_InitCVVDP_1,resizeToDisplay); FIELD(Vship_InitCVVDP_1,model_key_cstr); FIELD(Vship_InitCVVDP_1,model_config_json_cstr); FIELD(Vship_InitCVVDP_1,gpu_id);
    TYPE(Vship_ScoreSSIMULACRA2); FIELD(Vship_ScoreSSIMULACRA2,structType); FIELD(Vship_ScoreSSIMULACRA2,score);
    TYPE(Vship_ScoreButteraugli); FIELD(Vship_ScoreButteraugli,structType); FIELD(Vship_ScoreButteraugli,normQ); FIELD(Vship_ScoreButteraugli,norm3); FIELD(Vship_ScoreButteraugli,norminf); FIELD(Vship_ScoreButteraugli,dstp); FIELD(Vship_ScoreButteraugli,dststride);
    TYPE(Vship_ScoreCVVDP); FIELD(Vship_ScoreCVVDP,structType); FIELD(Vship_ScoreCVVDP,score); FIELD(Vship_ScoreCVVDP,dstp); FIELD(Vship_ScoreCVVDP,dststride);
    TYPE(Vship_DeviceInfo); FIELD(Vship_DeviceInfo,name); FIELD(Vship_DeviceInfo,VRAMSize); FIELD(Vship_DeviceInfo,integrated); FIELD(Vship_DeviceInfo,MultiProcessorCount); FIELD(Vship_DeviceInfo,WarpSize); FIELD(Vship_DeviceInfo,vulkanFeatureMatrix);
    printf("%d %d %d %d %d %d %d %d %d %d %d %d %d\n",Vship_NoError,Vship_BadErrorType,Vship_HIP,Vship_Cuda,Vship_Vulkan,Vship_CPU,Vship_StructNull,Vship_StructInitSSIMULACRA2_1,Vship_StructInitButteraugli_1,Vship_StructInitCVVDP_1,Vship_StructScoreSSIMULACRA2,Vship_StructScoreButteraugli,Vship_StructScoreCVVDP);
    return 0;
}
#endif
