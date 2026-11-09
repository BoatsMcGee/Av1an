#include <dav1d/headers.h>
#include <dav1d/version.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>

#if DAV1D_API_VERSION_MAJOR != 7
#error "Unsupported dav1d header: review Dav1dFilmGrainData ABI before updating this probe"
#endif

int main(void) {
    Dav1dFilmGrainData data;
    /* Initialize C padding too; never use an uninitialized object-byte oracle. */
    memset(&data, 0, sizeof(data));
    data.seed = 0x01234567u;
    data.num_y_points = 2;
    for (int i = 0; i < 14; ++i) {
        data.y_points[i][0] = 17;
        data.y_points[i][1] = 29;
    }
    data.y_points[0][0] = 1;
    data.y_points[0][1] = 2;
    data.y_points[13][0] = 253;
    data.y_points[13][1] = 254;
    data.chroma_scaling_from_luma = -3;
    data.num_uv_points[0] = 4;
    data.num_uv_points[1] = 5;
    for (int i = 0; i < 10; ++i) {
        data.uv_points[0][i][0] = 31;
        data.uv_points[0][i][1] = 47;
        data.uv_points[1][i][0] = 61;
        data.uv_points[1][i][1] = 79;
    }
    data.uv_points[0][9][0] = 251;
    data.uv_points[0][9][1] = 252;
    data.uv_points[1][9][0] = 253;
    data.uv_points[1][9][1] = 254;
    data.scaling_shift = -17;
    data.ar_coeff_lag = 3;
    for (int i = 0; i < 24; ++i) data.ar_coeffs_y[i] = -128;
    data.ar_coeffs_y[23] = 127;
    for (int i = 0; i < 28; ++i) {
        data.ar_coeffs_uv[0][i] = -127;
        data.ar_coeffs_uv[1][i] = 126;
    }
    data.ar_coeffs_uv[0][27] = 127;
    data.ar_coeffs_uv[1][27] = -128;
    data.ar_coeff_shift = UINT64_C(0x0123456789abcdef);
    data.grain_scale_shift = -23;
    data.uv_mult[0] = -31;
    data.uv_mult[1] = 37;
    data.uv_luma_mult[0] = 41;
    data.uv_luma_mult[1] = -43;
    data.uv_offset[0] = -47;
    data.uv_offset[1] = 53;
    data.overlap_flag = 1;
    data.clip_to_restricted_range = -59;

    printf("layout %zu %zu %zu %zu\n", sizeof(data), _Alignof(Dav1dFilmGrainData), sizeof(int), sizeof(unsigned));
    printf("api %d %d %d\n", DAV1D_API_VERSION_MAJOR, DAV1D_API_VERSION_MINOR, DAV1D_API_VERSION_PATCH);
#define FIELD(name) printf(#name " %zu %zu\n", offsetof(Dav1dFilmGrainData, name), sizeof(data.name))
    FIELD(seed);
    FIELD(num_y_points);
    FIELD(y_points);
    FIELD(chroma_scaling_from_luma);
    FIELD(num_uv_points);
    FIELD(uv_points);
    FIELD(scaling_shift);
    FIELD(ar_coeff_lag);
    FIELD(ar_coeffs_y);
    FIELD(ar_coeffs_uv);
    FIELD(ar_coeff_shift);
    FIELD(grain_scale_shift);
    FIELD(uv_mult);
    FIELD(uv_luma_mult);
    FIELD(uv_offset);
    FIELD(overlap_flag);
    FIELD(clip_to_restricted_range);
#undef FIELD
    const unsigned char *bytes = (const unsigned char *)&data;
    for (size_t i = 0; i < sizeof(data); ++i) printf("%02x", bytes[i]);
    putchar('\n');
    return 0;
}
