// A minimal test harness: CHECK records a failure and carries on; main returns the verdict.
#ifndef CHECK_H
#define CHECK_H

#include <stdio.h>

static int check_failures;

#define CHECK(condition)                                                                       \
    do {                                                                                       \
        if (!(condition)) {                                                                    \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #condition);      \
            check_failures++;                                                                  \
        }                                                                                      \
    } while (0)

#define CHECK_RESULT() (check_failures == 0 ? 0 : 1)

#endif
