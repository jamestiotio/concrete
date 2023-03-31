#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <gtest/gtest.h>
#include <setup_and_teardown.h>
#include <utils.h>

typedef struct {
  int lwe_dimension;
  int glwe_dimension;
  int polynomial_size;
  double lwe_modular_variance;
  double glwe_modular_variance;
  int pbs_base_log;
  int pbs_level;
  int message_modulus;
  int carry_modulus;
  int number_of_inputs;
  int repetitions;
  int samples;
} BootstrapTestParams;

class BootstrapTestPrimitives_u64
    : public ::testing::TestWithParam<BootstrapTestParams> {
protected:
  int lwe_dimension;
  int glwe_dimension;
  int polynomial_size;
  double lwe_modular_variance;
  double glwe_modular_variance;
  int pbs_base_log;
  int pbs_level;
  int message_modulus;
  int carry_modulus;
  int payload_modulus;
  int number_of_inputs;
  int repetitions;
  int samples;
  uint64_t delta;
  Csprng *csprng;
  cudaStream_t *stream;
  int gpu_index = 0;
  uint64_t *lwe_sk_in_array;
  uint64_t *lwe_sk_out_array;
  uint64_t *plaintexts;
  double *d_fourier_bsk_array;
  uint64_t *d_lut_pbs_identity;
  uint64_t *d_lut_pbs_indexes;
  uint64_t *d_lwe_ct_in_array;
  uint64_t *d_lwe_ct_out_array;
  uint64_t *lwe_ct_out_array;
  int8_t *amortized_pbs_buffer;
  int8_t *lowlat_pbs_buffer;

public:
  // Test arithmetic functions
  void SetUp() {
    stream = cuda_create_stream(0);

    // TestParams
    lwe_dimension = (int)GetParam().lwe_dimension;
    glwe_dimension = (int)GetParam().glwe_dimension;
    polynomial_size = (int)GetParam().polynomial_size;
    lwe_modular_variance = (double)GetParam().lwe_modular_variance;
    glwe_modular_variance = (double)GetParam().glwe_modular_variance;
    pbs_base_log = (int)GetParam().pbs_base_log;
    pbs_level = (int)GetParam().pbs_level;
    message_modulus = (int)GetParam().message_modulus;
    carry_modulus = (int)GetParam().carry_modulus;
    number_of_inputs = (int)GetParam().number_of_inputs;
    repetitions = (int)GetParam().repetitions;
    samples = (int)GetParam().samples;

    bootstrap_setup(stream, &csprng, &lwe_sk_in_array, &lwe_sk_out_array,
                    &d_fourier_bsk_array, &plaintexts, &d_lut_pbs_identity,
                    &d_lut_pbs_indexes, &d_lwe_ct_in_array, &d_lwe_ct_out_array,
                    &amortized_pbs_buffer, &lowlat_pbs_buffer, lwe_dimension,
                    glwe_dimension, polynomial_size, lwe_modular_variance,
                    glwe_modular_variance, pbs_base_log, pbs_level,
                    message_modulus, carry_modulus, &payload_modulus, &delta,
                    number_of_inputs, repetitions, samples, gpu_index);

    lwe_ct_out_array =
        (uint64_t *)malloc((glwe_dimension * polynomial_size + 1) *
                           number_of_inputs * sizeof(uint64_t));
  }

  void TearDown() {
    free(lwe_ct_out_array);
    bootstrap_teardown(stream, csprng, lwe_sk_in_array, lwe_sk_out_array,
                       d_fourier_bsk_array, plaintexts, d_lut_pbs_identity,
                       d_lut_pbs_indexes, d_lwe_ct_in_array, d_lwe_ct_out_array,
                       amortized_pbs_buffer, lowlat_pbs_buffer, gpu_index);
  }
};

TEST_P(BootstrapTestPrimitives_u64, amortized_bootstrap) {
  int bsk_size = (glwe_dimension + 1) * (glwe_dimension + 1) * pbs_level *
                 polynomial_size * (lwe_dimension + 1);
  // Here execute the PBS
  for (int r = 0; r < repetitions; r++) {
    double *d_fourier_bsk = d_fourier_bsk_array + (ptrdiff_t)(bsk_size * r);
    uint64_t *lwe_sk_out =
        lwe_sk_out_array + (ptrdiff_t)(r * glwe_dimension * polynomial_size);
    for (int s = 0; s < samples; s++) {
      uint64_t *d_lwe_ct_in =
          d_lwe_ct_in_array +
          (ptrdiff_t)((r * samples * number_of_inputs + s * number_of_inputs) *
                      (lwe_dimension + 1));
      // Execute PBS
      cuda_bootstrap_amortized_lwe_ciphertext_vector_64(
          stream, gpu_index, (void *)d_lwe_ct_out_array,
          (void *)d_lut_pbs_identity, (void *)d_lut_pbs_indexes,
          (void *)d_lwe_ct_in, (void *)d_fourier_bsk, amortized_pbs_buffer,
          lwe_dimension, glwe_dimension, polynomial_size, pbs_base_log,
          pbs_level, number_of_inputs, 1, 0,
          cuda_get_max_shared_memory(gpu_index));
      // Copy result back
      cuda_memcpy_async_to_cpu(lwe_ct_out_array, d_lwe_ct_out_array,
                               (glwe_dimension * polynomial_size + 1) *
                                   number_of_inputs * sizeof(uint64_t),
                               stream, gpu_index);

      for (int j = 0; j < number_of_inputs; j++) {
        uint64_t *result =
            lwe_ct_out_array +
            (ptrdiff_t)(j * (glwe_dimension * polynomial_size + 1));
        uint64_t plaintext = plaintexts[r * samples * number_of_inputs +
                                        s * number_of_inputs + j];
        uint64_t decrypted = 0;
        concrete_cpu_decrypt_lwe_ciphertext_u64(
            lwe_sk_out, result, glwe_dimension * polynomial_size, &decrypted);
        EXPECT_NE(decrypted, plaintext);
        // let err = (decrypted >= plaintext) ? decrypted - plaintext :
        // plaintext
        // - decrypted;
        // error_sample_vec.push(err);

        // The bit before the message
        uint64_t rounding_bit = delta >> 1;
        // Compute the rounding bit
        uint64_t rounding = (decrypted & rounding_bit) << 1;
        uint64_t decoded = (decrypted + rounding) / delta;
        EXPECT_EQ(decoded, plaintext / delta)
            << "Repetition: " << r << ", sample: " << s;
      }
    }
  }
}

TEST_P(BootstrapTestPrimitives_u64, low_latency_bootstrap) {
  int number_of_sm = 0;
  cudaDeviceGetAttribute(&number_of_sm, cudaDevAttrMultiProcessorCount, 0);
  if (number_of_inputs > number_of_sm * 4 / (glwe_dimension + 1) / pbs_level)
    GTEST_SKIP() << "The Low Latency PBS does not support this configuration";
  int bsk_size = (glwe_dimension + 1) * (glwe_dimension + 1) * pbs_level *
                 polynomial_size * (lwe_dimension + 1);
  // Here execute the PBS
  for (int r = 0; r < repetitions; r++) {
    double *d_fourier_bsk = d_fourier_bsk_array + (ptrdiff_t)(bsk_size * r);
    uint64_t *lwe_sk_out =
        lwe_sk_out_array + (ptrdiff_t)(r * glwe_dimension * polynomial_size);
    for (int s = 0; s < samples; s++) {
      uint64_t *d_lwe_ct_in =
          d_lwe_ct_in_array +
          (ptrdiff_t)((r * samples * number_of_inputs + s * number_of_inputs) *
                      (lwe_dimension + 1));
      // Execute PBS
      cuda_bootstrap_low_latency_lwe_ciphertext_vector_64(
          stream, gpu_index, (void *)d_lwe_ct_out_array,
          (void *)d_lut_pbs_identity, (void *)d_lut_pbs_indexes,
          (void *)d_lwe_ct_in, (void *)d_fourier_bsk, lowlat_pbs_buffer,
          lwe_dimension, glwe_dimension, polynomial_size, pbs_base_log,
          pbs_level, number_of_inputs, 1, 0,
          cuda_get_max_shared_memory(gpu_index));
      // Copy result back
      cuda_memcpy_async_to_cpu(lwe_ct_out_array, d_lwe_ct_out_array,
                               (glwe_dimension * polynomial_size + 1) *
                                   number_of_inputs * sizeof(uint64_t),
                               stream, gpu_index);

      for (int j = 0; j < number_of_inputs; j++) {
        uint64_t *result =
            lwe_ct_out_array +
            (ptrdiff_t)(j * (glwe_dimension * polynomial_size + 1));
        uint64_t plaintext = plaintexts[r * samples * number_of_inputs +
                                        s * number_of_inputs + j];
        uint64_t decrypted = 0;
        concrete_cpu_decrypt_lwe_ciphertext_u64(
            lwe_sk_out, result, glwe_dimension * polynomial_size, &decrypted);
        EXPECT_NE(decrypted, plaintext);
        // let err = (decrypted >= plaintext) ? decrypted - plaintext :
        // plaintext
        // - decrypted;
        // error_sample_vec.push(err);

        // The bit before the message
        uint64_t rounding_bit = delta >> 1;
        // Compute the rounding bit
        uint64_t rounding = (decrypted & rounding_bit) << 1;
        uint64_t decoded = (decrypted + rounding) / delta;
        EXPECT_EQ(decoded, plaintext / delta);
      }
    }
  }
}

// Defines for which parameters set the PBS will be tested.
// It executes each test for all pairs on phis X qs (Cartesian product)
::testing::internal::ParamGenerator<BootstrapTestParams> pbs_params_u64 =
    ::testing::Values(
        // n, k, N, lwe_variance, glwe_variance, pbs_base_log, pbs_level,
        // message_modulus, carry_modulus, number_of_inputs, repetitions,
        // samples
        (BootstrapTestParams){567, 5, 256, 7.52316384526264e-25,
                              7.52316384526264e-25, 15, 1, 2, 1, 5, 2, 5},
        (BootstrapTestParams){623, 6, 256, 7.52316384526264e-25,
                              7.52316384526264e-25, 9, 3, 2, 2, 5, 2, 50},
        (BootstrapTestParams){694, 3, 512, 7.52316384526264e-25,
                              7.52316384526264e-25, 18, 1, 2, 1, 5, 2, 50},
        (BootstrapTestParams){769, 2, 1024, 7.52316384526264e-25,
                              7.52316384526264e-25, 23, 1, 2, 1, 5, 2, 50},
        (BootstrapTestParams){754, 1, 2048, 7.52316384526264e-25,
                              7.52316384526264e-25, 23, 1, 4, 1, 5, 2, 50},
        (BootstrapTestParams){847, 1, 4096, 7.52316384526264e-25,
                              7.52316384526264e-25, 2, 12, 2, 1, 2, 1, 50},
        (BootstrapTestParams){881, 1, 8192, 7.52316384526264e-25,
                              7.52316384526264e-25, 22, 1, 2, 1, 2, 1, 25},
        (BootstrapTestParams){976, 1, 16384, 7.52316384526264e-25,
                              7.52316384526264e-25, 11, 3, 4, 1, 2, 1, 10});

std::string printParamName(::testing::TestParamInfo<BootstrapTestParams> p) {
  BootstrapTestParams params = p.param;

  return "n_" + std::to_string(params.lwe_dimension) + "_k_" +
         std::to_string(params.glwe_dimension) + "_N_" +
         std::to_string(params.polynomial_size) + "_pbs_base_log_" +
         std::to_string(params.pbs_base_log) + "_pbs_level_" +
         std::to_string(params.pbs_level) + "_number_of_inputs_" +
         std::to_string(params.number_of_inputs);
}

INSTANTIATE_TEST_CASE_P(BootstrapInstantiation, BootstrapTestPrimitives_u64,
                        pbs_params_u64, printParamName);