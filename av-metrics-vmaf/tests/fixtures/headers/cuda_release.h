/**
 * Copyright 2016-2023 Netflix, Inc.
 * Copyright 2021 NVIDIA Corporation.
 *
 * Licensed under the BSD+Patent License (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 * https://opensource.org/licenses/BSDplusPatent
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

/* Verbatim private declaration excerpt from libvmaf/src/cuda/common.h at
 * f85a853692a8c730d0270cd733c8bb30b5b93b7c. The whole private header requires
 * a CUDA SDK; only this opaque-pointer signature is used by our loader.
 * No shared struct definitions are reproduced here. */
int vmaf_cuda_release(VmafCudaState *cu_state);
