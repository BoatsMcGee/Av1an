#ifndef VSHIP_API_HEADER
#define VSHIP_API_HEADER

#include <stdint.h>
#include <stdbool.h>

#include "VshipColor.h"

#if defined(_WIN32)
#if defined(EXPORTVSHIPLIB)
#define EXPORTPREPROCESS __declspec(dllexport)
#else
#ifndef VSHIP_STATIC
#define EXPORTPREPROCESS __declspec(dllimport)
#else
#define EXPORTPREPROCESS
#endif
#endif
#else
#define EXPORTPREPROCESS
#endif

#ifdef __cplusplus
extern "C"{
#endif

typedef enum{
    Vship_HIP = 0,
    Vship_Cuda = 1,
    Vship_Vulkan = 2,
    Vship_CPU = 3,
} Vship_Backend;

typedef struct {
    int major;
    int minor;
    int minorMinor;
    Vship_Backend backend;
} Vship_Version;

EXPORTPREPROCESS Vship_Version Vship_GetVersion();

//this is general purpose, it contains error that cannot be encountered using the API
typedef enum{
    Vship_NoError = 0,

    //vship internal issues
    Vship_OutOfVRAM = 1,
    Vship_OutOfRAM = 2,
    Vship_HIPError = 12,
    
    //input issues
    Vship_BadDisplayModel = 3,
    Vship_DifferingInputType = 4,
    Vship_NonRGBSInput = 5, //should never happen since .resize should give RGBS always
    Vship_BadPath = 13,
    Vship_BadJson = 14,

    //Device related
    Vship_DeviceCountError = 6,
    Vship_NoDeviceDetected = 7,
    Vship_BadDeviceArgument = 8,
    Vship_BadDeviceCode = 9,

    //API related
    Vship_BadHandler = 10,
    Vship_BadPointer = 11,
    Vship_NotSupported = 15,
    Vship_BadInputStructType = 16,

    //should not be used
    Vship_BadErrorType = 17,
} Vship_Exception;

//Get the number of GPU
EXPORTPREPROCESS Vship_Exception Vship_GetDeviceCount(int* number);

//if set to false means that the gpu does not support it
typedef struct Vship_VulkanFeatureMatrix{
    bool storageBuffer16BitAccess;
    bool uniformAndStorageBuffer16BitAccess; //not required
    bool variablePointersStorageBuffer;
    bool variablePointers;
    bool storagePushConstant8; //not required
    bool storageBuffer8BitAccess;
    bool uniformAndStorageBuffer8BitAccess; //not required
    bool shaderFloat16; //not required, fp16 is disabled by default in vulkan build
    bool shaderFloat64; //not required
    bool shaderInt8;
    bool shaderInt16;
    bool shaderInt64;
    bool bufferDeviceAddress;
    bool shaderStorageBufferArrayDynamicIndexing;
    bool synchronization2;
    bool logicOp; //not required
    bool FutureFeatures[10]; //reserved for future use
} Vship_VulkanFeatureMatrix;

//more features might be added later on in this struct
typedef struct Vship_DeviceInfo{
    char name[256];
    uint64_t VRAMSize; //size in bytes
    int integrated; //iGPU? (boolean)
    int MultiProcessorCount;
    int WarpSize;
    Vship_VulkanFeatureMatrix vulkanFeatureMatrix; //only set for Vulkan
} Vship_DeviceInfo;

EXPORTPREPROCESS Vship_Exception Vship_GetDeviceInfo(Vship_DeviceInfo* device_info, int gpu_id);

//very useful function allowing to see if vship is going to work there are multiple errors possible returned
EXPORTPREPROCESS Vship_Exception Vship_GPUFullCheck(int gpu_id);

//you can allocate typically 1024 bytes to retrieve the full error message
//however, if you want the exact amount, the integer returned is the size needed. So you can use len=0 to retrieve the size, allocate and then the correct len.
EXPORTPREPROCESS int Vship_GetErrorMessage(Vship_Exception exception, char* out_message, int len);

//works exactly like the above function but return more details, only about the last error that happened
//for multithreaded scenarios, (ie one thread per handler) refer to the Handler versions of this function below
EXPORTPREPROCESS int Vship_GetDetailedLastError(char* out_message, int len);

EXPORTPREPROCESS Vship_Exception Vship_PinnedMalloc2(void** ptr, uint64_t size, int gpu_id);
EXPORTPREPROCESS Vship_Exception Vship_PinnedFree2(void* ptr, int gpu_id);

typedef void* Vship_Handle;

typedef enum{
    Vship_StructNull = 0,
    Vship_StructInitSSIMULACRA2_1 = 1,
    Vship_StructInitButteraugli_1 = 2,
    Vship_StructInitCVVDP_1 = 3,
    Vship_StructScoreSSIMULACRA2 = 4,
    Vship_StructScoreButteraugli = 5,
    Vship_StructScoreCVVDP = 6,
} Vship_StructType;

typedef struct Vship_InitSSIMULACRA2_1{
    Vship_StructType structType; //should be Vship_StructInitSSIMULACRA2_1
    Vship_Colorspace_t src_colorspace;
    Vship_Colorspace_t dis_colorspace;
    int gpu_id;
} Vship_InitSSIMULACRA2_1;

typedef struct Vship_InitButteraugli_1{
    Vship_StructType structType; //should be Vship_StructInitButteraugli_1
    Vship_Colorspace_t src_colorspace;
    Vship_Colorspace_t dis_colorspace;
    int Qnorm;
    float intensity_multiplier;
    int gpu_id;
} Vship_InitButteraugli_1;

typedef struct Vship_InitCVVDP_1{
    Vship_StructType structType; //should be Vship_StructInitCVVDP_1
    Vship_Colorspace_t src_colorspace;
    Vship_Colorspace_t dis_colorspace;
    float fps;
    bool resizeToDisplay;
    const char* model_key_cstr;
    const char* model_config_json_cstr;
    int gpu_id;
} Vship_InitCVVDP_1;

//this function allows to create a handler for any metric using argument with
//handler pointer is returned as first argument, you must not modify its content
EXPORTPREPROCESS Vship_Exception Vship_InitHandler(Vship_Handle* handler, void* argument);

EXPORTPREPROCESS Vship_Exception Vship_FreeHandler(Vship_Handle handler);

//works exactly like Vship_GetDetailedLastError but returns the last error of a handler instead of being global which is better in multithreaded scenarios
//if it returns 0, it means an error occurent inside, you should use Vship_GetDetailedLastError
EXPORTPREPROCESS int Vship_GetDetailedLastErrorHandler(Vship_Handle handler, char* out_message, int len);

typedef struct Vship_ScoreSSIMULACRA2{
    Vship_StructType structType; //should be Vship_StructScoreSSIMULACRA2
    double score; //is written to
} Vship_ScoreSSIMULACRA2;

typedef struct Vship_ScoreButteraugli{
    Vship_StructType structType; //should be Vship_StructScoreButteraugli
    double normQ; //is written to
    double norm3; //is written to
    double norminf; //is written to
    //dstp must either be NULL (in this case, the distortion map will never be retrieved from the gpu)
    //or be allocated of size dststride*height
    const uint8_t *dstp;
    int64_t dststride; //is read
} Vship_ScoreButteraugli;

typedef struct Vship_ScoreCVVDP{
    Vship_StructType structType; //should be Vship_StructScoreCVVDP
    double score; //is written to
    //dstp must either be NULL (in this case, the distortion map will never be retrieved from the gpu)
    //or be allocated of size dststride*height
    const uint8_t *dstp;
    int64_t dststride; //is read
} Vship_ScoreCVVDP;

//score should be a struct like specified before depending on the metric
EXPORTPREPROCESS Vship_Exception Vship_ComputeHandler(Vship_Handle handler, void* score, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//in order to retrieve the distortion map, you might want to know its resolution (it varies depending on parameters)
EXPORTPREPROCESS Vship_Exception Vship_GetDistmapResolution(Vship_Handle handler, int64_t* width, int64_t* height);

//this function empties temporal filter history of temporal metrics (but not score accumulation, see below)
EXPORTPREPROCESS Vship_Exception Vship_Reset(Vship_Handle handler);

//This function is to not empty the temporal filter history but the score accumulation (useful to get per scene scores)
EXPORTPREPROCESS Vship_Exception Vship_ResetScore(Vship_Handle handler);

//This function allows loading frames to the temporal filter to represent past frames without impacting the score for CVVDP
EXPORTPREPROCESS Vship_Exception Vship_LoadTemporalCVVDP2(Vship_Handle handler, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//------------------------------------------------------------------
//DEPRECATION WARNING: everything after this mark only constitutes backward compatibility
//------------------------------------------------------------------

//only useful for old init that are themselves deprecated, new handler init struct requires gpu_id to be specified
//default device is default by HIP/Cuda (0)
EXPORTPREPROCESS Vship_Exception Vship_SetDevice(int gpu_id);

//now we ask for gpu_id here too (useful for vulkan specifically right now)
EXPORTPREPROCESS Vship_Exception Vship_PinnedMalloc(void** ptr, uint64_t size);
EXPORTPREPROCESS Vship_Exception Vship_PinnedFree(void* ptr);

//for maximum throughput, it is recommend to use 3 SSIMU2Handler with each a thread to use in parallel
//is only an id to refer to an object in an array in the API dll.
//this is because the original object contains types that are not represented without hip and the original code.
typedef struct Vship_SSIMU2Handler{
    unsigned int id;
} Vship_SSIMU2Handler;

//handler pointer will be replaced, it is a return value. Don't forget to free it after usage.
EXPORTPREPROCESS Vship_Exception Vship_SSIMU2Init(Vship_SSIMU2Handler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace);

EXPORTPREPROCESS Vship_Exception Vship_SSIMU2Init2(Vship_SSIMU2Handler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, int gpu_id);

//handler pointer can be discarded after this function.
EXPORTPREPROCESS Vship_Exception Vship_SSIMU2Free(Vship_SSIMU2Handler handler);

//the frame is not overwritten
EXPORTPREPROCESS Vship_Exception Vship_ComputeSSIMU2(Vship_SSIMU2Handler handler, double* score, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//works exactly like Vship_GetDetailedLastError but returns the last error of a handler instead of being global which is better in multithreaded scenarios
//if it returns 0, it means an error occurent inside, you should use Vship_GetDetailedLastError
EXPORTPREPROCESS int Vship_SSIMU2GetDetailedLastError(Vship_SSIMU2Handler handler, char* out_message, int len);

typedef struct Vship_ButteraugliHandler{
    unsigned int id;
} Vship_ButteraugliHandler;

typedef struct Vship_ButteraugliScore{
    double normQ;
    double norm3;
    double norminf;
} Vship_ButteraugliScore;

//handler pointer will be replaced, it is a return value. Don't forget to free it after usage.
EXPORTPREPROCESS Vship_Exception Vship_ButteraugliInit(Vship_ButteraugliHandler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, int Qnorm, float intensity_multiplier);

EXPORTPREPROCESS Vship_Exception Vship_ButteraugliInit2(Vship_ButteraugliHandler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, int Qnorm, float intensity_multiplier, int gpu_id);

//handler pointer can be discarded after this function.
EXPORTPREPROCESS Vship_Exception Vship_ButteraugliFree(Vship_ButteraugliHandler handler);

//the frame is not overwritten
//dstp must either be NULL (in this case, the distortion map will never be retrieved from the gpu)
//or be allocated of size dststride*height
//output in score
EXPORTPREPROCESS Vship_Exception Vship_ComputeButteraugli(Vship_ButteraugliHandler handler, Vship_ButteraugliScore* score, const uint8_t *dstp, int64_t dststride, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//works exactly like Vship_GetDetailedLastError but returns the last error of a handler instead of being global which is better in multithreaded scenarios
//if it returns 0, it means an error occurent inside, you should use Vship_GetDetailedLastError
EXPORTPREPROCESS int Vship_ButteraugliGetDetailedLastError(Vship_ButteraugliHandler handler, char* out_message, int len);

typedef struct Vship_CVVDPHandler{
    unsigned int id;
} Vship_CVVDPHandler;

//handler pointer will be replaced, it is a return value. Don't forget to free it after usage.
EXPORTPREPROCESS Vship_Exception Vship_CVVDPInit(Vship_CVVDPHandler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, float fps, bool resizeToDisplay, const char* model_key_cstr);

//allows specifying a path for a json containing display informations.
//basically properties will be searched through basic models and then overwritten by the custom model
// (so you can overwrite only some properties of an existing model if you wish to)
//however if you go with a completely new display name, you will have to specify everything
//passing NULL allows to have no path (similarly "\0" works)
EXPORTPREPROCESS Vship_Exception Vship_CVVDPInit2(Vship_CVVDPHandler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, float fps, bool resizeToDisplay, const char* model_key_cstr, const char* model_config_json_cstr);

EXPORTPREPROCESS Vship_Exception Vship_CVVDPInit3(Vship_CVVDPHandler* handler, Vship_Colorspace_t src_colorspace, Vship_Colorspace_t dis_colorspace, float fps, bool resizeToDisplay, const char* model_key_cstr, const char* model_config_json_cstr, int gpu_id);

//handler pointer can be discarded after this function.
EXPORTPREPROCESS Vship_Exception Vship_CVVDPFree(Vship_CVVDPHandler handler);

//in order to retrieve the distortion map, you might want to know its resolution (it varies depending on parameters)
EXPORTPREPROCESS Vship_Exception Vship_CVVDPGetDistmapResolution(Vship_CVVDPHandler handler, int64_t* width, int64_t* height);

//Allows to empty the frame history. Since the metric is temporal, you may want to reset without recreating a whole new handler since it is quite expensive
//Unlike recreating the handler, this function is basically free perf wise, it is setting a variable to 0, that's it!
//only reset temporal filter but not score accumulation! to reset both you need to use both this function and below
EXPORTPREPROCESS Vship_Exception Vship_ResetCVVDP(Vship_CVVDPHandler handler);

//This function is to not empty the temporal filter history but the score (useful to get per scene scores)
EXPORTPREPROCESS Vship_Exception Vship_ResetScoreCVVDP(Vship_CVVDPHandler handler);

EXPORTPREPROCESS Vship_Exception Vship_LoadTemporalCVVDP(Vship_CVVDPHandler handler, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//dstp must either be NULL (in this case, the distortion map will never be retrieved from the gpu)
//or be allocated of size dststride*height where height is original height or display height if resizeToDisplay is on
//output the score of the whole sequence that it has already seen. You can reset the CVVDP handler to start over on a new sequence
//ideally, for a video, you feed all the frames, and then only at the very last frame submitted you take the score
EXPORTPREPROCESS Vship_Exception Vship_ComputeCVVDP(Vship_CVVDPHandler handler, double* score, const uint8_t *dstp, int64_t dststride, const uint8_t* srcp1[3], const uint8_t* srcp2[3], const int64_t lineSize[3], const int64_t lineSize2[3]);

//works exactly like Vship_GetDetailedLastError but returns the last error of a handler instead of being global which is better in multithreaded scenarios
//if it returns 0, it means an error occurent inside, you should use Vship_GetDetailedLastError
EXPORTPREPROCESS int Vship_CVVDPGetDetailedLastError(Vship_CVVDPHandler handler, char* out_message, int len);

#ifdef __cplusplus
} //extern "C"
#endif
#endif //ifndef VSHIP_API_HEADER
