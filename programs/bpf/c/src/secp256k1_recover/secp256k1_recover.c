/**
 * @brief Secp256k1Recover Syscall test
 */
#include <solana_sdk.h>

extern uint64_t entrypoint(const uint8_t *input) {

  uint8_t result[SECP256K1_RECOVER_RESULT_LENGTH];
  uint8_t expected[] = { 0x42, 0xcd, 0x27, 0xe4, 0x0f, 0xdf, 0x7c, 0x97,
                         0x0a, 0xa2, 0xca, 0x0b, 0x88, 0x5b, 0x96, 0x0f,
                         0x8b, 0x62, 0x8a, 0x41, 0xa1, 0x81, 0xe7, 0xe6,
                         0x8e, 0x03, 0xea, 0x0b, 0x84, 0x20, 0x58, 0x9b,
                         0x32, 0x06, 0xbd, 0x66, 0x2f, 0x75, 0x65, 0xd6,
                         0x9d, 0xbd, 0x1d, 0x34, 0x29, 0x6a, 0xd9, 0x35,
                         0x38, 0xed, 0x86, 0x9e, 0x99, 0x20, 0x43, 0xc3,
                         0xeb, 0xad, 0x65, 0x50, 0xa0, 0x11, 0x6e, 0x5d};

  uint8_t hash[] = { 0xde, 0xa5, 0x66, 0xb6, 0x94, 0x3b, 0xe0, 0xe9,
                          0x62, 0x53, 0xc2, 0x21, 0x5b, 0x1b, 0xac, 0x69,
                          0xe7, 0xa8, 0x1e, 0xdb, 0x41, 0xc5, 0x02, 0x8b,
                          0x4f, 0x5c, 0x45, 0xc5, 0x3b, 0x49, 0x54, 0xd0};
  uint8_t signature[] = { 0x97, 0xa4, 0xee, 0x31, 0xfe, 0x82, 0x65, 0x72,
                          0x9f, 0x4a, 0xa6, 0x7d, 0x24, 0xd4, 0xa7, 0x27,
                          0xf8, 0xc3, 0x15, 0xa4, 0xc8, 0xf9, 0x80, 0xeb,
                          0x4c, 0x4d, 0x4a, 0xfa, 0x6e, 0xc9, 0x42, 0x41,
                          0x5d, 0x10, 0xd9, 0xc2, 0x8a, 0x90, 0xe9, 0x92,
                          0x9c, 0x52, 0x4b, 0x2c, 0xfb, 0x65, 0xdf, 0xbc,
                          0xf6, 0x8c, 0xfd, 0x68, 0xdb, 0x17, 0xf9, 0x5d,
                          0x23, 0x5f, 0x96, 0xd8, 0xf0, 0x72, 0x01, 0x2d};
  uint64_t recovery_id = 1;

  uint64_t result_code = sol_secp256k1_recover(hash, recovery_id, signature, result);

  sol_assert(0 == result_code);
  sol_assert(0 == sol_memcmp(result, expected, SHA256_RESULT_LENGTH));

  return SUCCESS;
}
