#define _POSIX_C_SOURCE 200809L
#include <arpa/inet.h>
#include <errno.h>
#include <math.h>
#include <maxminddb.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#include <pthread.h>


/*
 * When linked with -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc these counters
 * include allocations made by the statically linked libmaxminddb as well as the
 * benchmark executable. They are reset immediately before the bulk measured loop.
 */
static _Thread_local uint64_t allocation_calls = 0;
static _Thread_local uint64_t allocated_bytes = 0;

void *__real_malloc(size_t);
void *__real_calloc(size_t, size_t);
void *__real_realloc(void *, size_t);

void *__wrap_malloc(size_t size) {
    allocation_calls += 1;
    allocated_bytes += (uint64_t)size;
    return __real_malloc(size);
}

void *__wrap_calloc(size_t n, size_t size) {
    allocation_calls += 1;
    if (n != 0 && size <= SIZE_MAX / n) {
        allocated_bytes += (uint64_t)(n * size);
    }
    return __real_calloc(n, size);
}

void *__wrap_realloc(void *ptr, size_t size) {
    allocation_calls += 1;
    allocated_bytes += (uint64_t)size;
    return __real_realloc(ptr, size);
}

static uint64_t ns_now(clockid_t clock_id) {
    struct timespec ts;
    if (clock_gettime(clock_id, &ts) != 0) return 0;
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

static uint64_t current_rss_bytes(void) {
    FILE *f = fopen("/proc/self/statm", "r");
    if (!f) return 0;
    unsigned long total_pages = 0, rss_pages = 0;
    int ok = fscanf(f, "%lu %lu", &total_pages, &rss_pages);
    fclose(f);
    if (ok != 2) return 0;
    long page = sysconf(_SC_PAGESIZE);
    if (page <= 0) return 0;
    return (uint64_t)rss_pages * (uint64_t)page;
}

static uint64_t peak_rss_bytes(void) {
    struct rusage usage;
    if (getrusage(RUSAGE_SELF, &usage) != 0) return 0;
#if defined(__APPLE__)
    return (uint64_t)usage.ru_maxrss;
#else
    return (uint64_t)usage.ru_maxrss * 1024ULL;
#endif
}

static int cmp_u64(const void *a, const void *b) {
    uint64_t x = *(const uint64_t *)a;
    uint64_t y = *(const uint64_t *)b;
    return (x > y) - (x < y);
}

typedef struct {
    double mean;
    double median;
    uint64_t min;
    uint64_t max;
    uint64_t p50;
    uint64_t p95;
    uint64_t p99;
    double variance;
    double stddev;
} stats_t;

static uint64_t percentile(const uint64_t *sorted, size_t n, double q) {
    if (n == 0) return 0;
    size_t idx = (size_t)llround((double)(n - 1) * q);
    if (idx >= n) idx = n - 1;
    return sorted[idx];
}

static stats_t summarize(uint64_t *samples, size_t n) {
    qsort(samples, n, sizeof(*samples), cmp_u64);
    long double sum = 0.0L;
    for (size_t i = 0; i < n; ++i) sum += (long double)samples[i];
    long double mean = n ? sum / (long double)n : 0.0L;
    long double variance = 0.0L;
    for (size_t i = 0; i < n; ++i) {
        long double d = (long double)samples[i] - mean;
        variance += d * d;
    }
    if (n) variance /= (long double)n;
    stats_t s = {
        .mean = (double)mean,
        .median = (double)percentile(samples, n, 0.50),
        .min = n ? samples[0] : 0,
        .max = n ? samples[n - 1] : 0,
        .p50 = percentile(samples, n, 0.50),
        .p95 = percentile(samples, n, 0.95),
        .p99 = percentile(samples, n, 0.99),
        .variance = (double)variance,
        .stddev = sqrt((double)variance),
    };
    return s;
}

static double timer_overhead_ns(void) {
    const size_t n = 10000;
    long double sum = 0.0L;
    for (size_t i = 0; i < n; ++i) {
        uint64_t start = ns_now(CLOCK_MONOTONIC);
        __asm__ __volatile__("" ::: "memory");
        sum += (long double)(ns_now(CLOCK_MONOTONIC) - start);
    }
    return (double)(sum / (long double)n);
}

typedef union {
    struct sockaddr_in v4;
    struct sockaddr_in6 v6;
} bench_addr_t;

static int parse_sockaddr(const char *ip, bench_addr_t *addr) {
    memset(addr, 0, sizeof(*addr));
    addr->v4.sin_family = AF_INET;
    if (inet_pton(AF_INET, ip, &addr->v4.sin_addr) == 1) return 0;

    memset(addr, 0, sizeof(*addr));
    addr->v6.sin6_family = AF_INET6;
    if (inet_pton(AF_INET6, ip, &addr->v6.sin6_addr) == 1) return 0;
    return -1;
}

typedef struct {
    bench_addr_t *items;
    bench_addr_t hot;
    size_t len;
    int is_hot;
} workload_t;

static void workload_free(workload_t *workload) {
    free(workload->items);
    workload->items = NULL;
}

static const struct sockaddr *bench_addr_sockaddr(const bench_addr_t *addr) {
    return addr->v4.sin_family == AF_INET
        ? (const struct sockaddr *)&addr->v4
        : (const struct sockaddr *)&addr->v6;
}

static const struct sockaddr *workload_get(const workload_t *workload, size_t i) {
    if (workload->is_hot) return bench_addr_sockaddr(&workload->hot);
    return bench_addr_sockaddr(&workload->items[i]);
}

static int load_workload(workload_t *out, const char *dir, const char *family,
                         const char *pattern, size_t size) {
    memset(out, 0, sizeof(*out));
    out->len = size;
    if (strcmp(pattern, "hot") == 0) {
        const char *ip = strcmp(family, "ipv6") == 0 ? getenv("BENCH_HOT_IPV6") : getenv("BENCH_HOT_IPV4");
        if (!ip || !*ip) ip = strcmp(family, "ipv6") == 0 ? "2001:db8:123::1" : "81.2.69.160";
        if (parse_sockaddr(ip, &out->hot) != 0) return -1;
        out->is_hot = 1;
        return 0;
    }

    char path[4096];
    int n = snprintf(path, sizeof(path), "%s/%s-%s.txt", dir, family, pattern);
    if (n <= 0 || (size_t)n >= sizeof(path)) return -1;
    FILE *f = fopen(path, "r");
    if (!f) return -1;

    out->items = calloc(size, sizeof(*out->items));
    if (!out->items) {
        fclose(f);
        return -1;
    }

    char *line = NULL;
    size_t cap = 0;
    ssize_t read = 0;
    size_t count = 0;
    while (count < size && (read = getline(&line, &cap, f)) >= 0) {
        while (read > 0 && (line[read - 1] == '\n' || line[read - 1] == '\r')) line[--read] = '\0';
        if (read == 0) continue;
        if (parse_sockaddr(line, &out->items[count]) != 0) {
            free(line);
            fclose(f);
            workload_free(out);
            return -1;
        }
        count++;
    }
    free(line);
    fclose(f);
    if (count != size) {
        workload_free(out);
        return -1;
    }
    return 0;
}

static uint64_t dataset_size(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return 0;
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return 0;
    }
    long size = ftell(f);
    fclose(f);
    return size > 0 ? (uint64_t)size : 0;
}

static volatile uint64_t sink = 0;

static uint64_t lookup_decode(MMDB_s *mmdb, const struct sockaddr *addr) {
    int mmdb_error = MMDB_SUCCESS;
    MMDB_lookup_result_s result = MMDB_lookup_sockaddr(mmdb, addr, &mmdb_error);
    if (mmdb_error != MMDB_SUCCESS || !result.found_entry) return 0;

    MMDB_entry_data_list_s *list = NULL;
    if (MMDB_get_entry_data_list(&result.entry, &list) != MMDB_SUCCESS || !list) return 0;
    uint64_t value = (uint64_t)result.found_entry ^ (uint64_t)(uintptr_t)list;
    MMDB_free_entry_data_list(list);
    sink ^= value;
    return value;
}

static void print_lookup_result(const char *scenario, size_t threads, const char *family, const char *pattern, size_t workload_size,
                                size_t warmup_ops, size_t measured_ops, uint64_t *samples, size_t sample_count,
                                uint64_t wall_ns, uint64_t cpu_ns, uint64_t allocations,
                                uint64_t bytes, uint64_t rss_before, uint64_t rss_after,
                                uint64_t peak_rss, uint64_t first_ns, uint64_t checksum,
                                const char *dataset) {
    stats_t stats = summarize(samples, sample_count);
    double throughput = wall_ns ? (double)measured_ops * 1e9 / (double)wall_ns : 0.0;
    double cpu_pct = wall_ns ? (double)cpu_ns * 100.0 / (double)wall_ns : 0.0;
    const char *revision = getenv("BENCH_IMPLEMENTATION_REVISION");
    if (!revision) revision = "unknown";
    printf(
        "{\"schema_version\":2,\"implementation\":\"libmaxminddb\",\"version\":\"%s\","
        "\"revision\":\"%s\",\"scenario\":\"%s\",\"operation\":\"lookup_decode\",\"threads\":%zu,"
        "\"family\":\"%s\",\"pattern\":\"%s\",\"workload_size\":%zu,"
        "\"warmup_ops\":%zu,\"measurement_operations\":%zu,\"latency_sample_count\":%zu,\"mean_ns\":%.3f,"
        "\"median_ns\":%.3f,\"min_ns\":%llu,\"max_ns\":%llu,\"p50_ns\":%llu,"
        "\"p95_ns\":%llu,\"p99_ns\":%llu,\"variance_ns2\":%.3f,\"stddev_ns\":%.3f,"
        "\"throughput_ops_s\":%.3f,\"wall_time_ns\":%llu,\"cpu_time_ns\":%llu,"
        "\"cpu_utilization_pct\":%.3f,\"allocation_calls\":%llu,\"allocations_per_op\":%.9f,"
        "\"allocated_bytes\":%llu,\"allocated_bytes_per_op\":%.3f,"
        "\"rss_before_bytes\":%llu,\"rss_after_bytes\":%llu,\"rss_delta_bytes\":%lld,"
        "\"peak_rss_bytes\":%llu,\"first_operation_ns\":%llu,\"timer_overhead_ns\":%.3f,"
        "\"dataset_bytes\":%llu,\"checksum\":%llu}\n",
        MMDB_lib_version(), revision, scenario, threads, family, pattern, workload_size, warmup_ops, measured_ops, sample_count,
        stats.mean, stats.median, (unsigned long long)stats.min, (unsigned long long)stats.max,
        (unsigned long long)stats.p50, (unsigned long long)stats.p95,
        (unsigned long long)stats.p99, stats.variance, stats.stddev, throughput,
        (unsigned long long)wall_ns, (unsigned long long)cpu_ns, cpu_pct,
        (unsigned long long)allocations, (double)allocations / (double)measured_ops,
        (unsigned long long)bytes, (double)bytes / (double)measured_ops,
        (unsigned long long)rss_before, (unsigned long long)rss_after,
        (long long)rss_after - (long long)rss_before, (unsigned long long)peak_rss,
        (unsigned long long)first_ns, timer_overhead_ns(),
        (unsigned long long)dataset_size(dataset), (unsigned long long)checksum);
}

static size_t env_size(const char *name, size_t default_value) {
    const char *value = getenv(name);
    if (!value || !*value) return default_value;
    char *end = NULL;
    errno = 0;
    unsigned long long parsed = strtoull(value, &end, 10);
    if (errno != 0 || end == value || (end && *end != '\0') || parsed == 0) return default_value;
    return (size_t)parsed;
}

static size_t env_size_allow_zero(const char *name, size_t default_value) {
    const char *value = getenv(name);
    if (!value || !*value) return default_value;
    char *end = NULL;
    errno = 0;
    unsigned long long parsed = strtoull(value, &end, 10);
    if (errno != 0 || end == value || (end && *end != '\0')) return default_value;
    return (size_t)parsed;
}

static uint64_t benchmark_duration_ns(const char *name) {
    size_t fallback = strcmp(name, "BENCH_WARMUP_SECS") == 0 ? 1 : 2;
    return (uint64_t)env_size(name, fallback) * UINT64_C(1000000000);
}

static int benchmark_lookup(const char *dataset) {
    const char *workload_dir = getenv("BENCH_WORKLOAD_DIR");
    const char *family = getenv("BENCH_FAMILY");
    const char *pattern = getenv("BENCH_PATTERN");
    if (!workload_dir || !family || !pattern) return 2;
    size_t size = env_size("BENCH_WORKLOAD_SIZE", 100000);
    uint64_t warmup_ns = benchmark_duration_ns("BENCH_WARMUP_SECS");
    uint64_t measure_ns = benchmark_duration_ns("BENCH_MEASURE_SECS");
    size_t sample_max = env_size("BENCH_LATENCY_SAMPLES_MAX", 100000);
    size_t sample_count = size < sample_max ? size : sample_max;

    workload_t workload;
    if (load_workload(&workload, workload_dir, family, pattern, size) != 0) return 3;

    MMDB_s mmdb;
    int status = MMDB_open(dataset, MMDB_MODE_MMAP, &mmdb);
    if (status != MMDB_SUCCESS) {
        workload_free(&workload);
        return 4;
    }

    /* Verify all misses before any timed lookup; errors are not valid misses. */
    if (strcmp(pattern, "absent") == 0) {
        for (size_t i = 0; i < size; i++) {
            int error = MMDB_SUCCESS;
            MMDB_lookup_result_s result = MMDB_lookup_sockaddr(&mmdb, workload_get(&workload, i), &error);
            if (error != MMDB_SUCCESS || result.found_entry) {
                fprintf(stderr, "absent preflight failed at query %zu (error %d)\n", i, error);
                MMDB_close(&mmdb);
                workload_free(&workload);
                return 5;
            }
        }
    }

    uint64_t first_start = ns_now(CLOCK_MONOTONIC);
    uint64_t first_value = lookup_decode(&mmdb, workload_get(&workload, 0));
    uint64_t first_ns = ns_now(CLOCK_MONOTONIC) - first_start;
    /* first_value may be 0 if first IP is not present */

    uint64_t warm_start = ns_now(CLOCK_MONOTONIC);
    uint64_t warm_checksum = first_value;
    size_t warmup = 0;
    while (ns_now(CLOCK_MONOTONIC) - warm_start < warmup_ns) {
        warm_checksum ^= lookup_decode(&mmdb, workload_get(&workload, warmup % size));
        warmup++;
    }
    sink ^= warm_checksum;

    uint64_t *samples = calloc(sample_count, sizeof(*samples));
    if (!samples) {
        MMDB_close(&mmdb);
        workload_free(&workload);
        return 6;
    }
    for (size_t i = 0; i < sample_count; ++i) {
        size_t index = sample_count == size ? i : (i * size) / sample_count;
        if (index >= size) index = size - 1;
        uint64_t start = ns_now(CLOCK_MONOTONIC);
        uint64_t value = lookup_decode(&mmdb, workload_get(&workload, index));
        samples[i] = ns_now(CLOCK_MONOTONIC) - start;
        sink ^= value;
    }

    uint64_t rss_before = current_rss_bytes();
    allocation_calls = 0;
    allocated_bytes = 0;
    uint64_t cpu_start = ns_now(CLOCK_PROCESS_CPUTIME_ID);
    uint64_t wall_start = ns_now(CLOCK_MONOTONIC);
    uint64_t checksum = 0;
    size_t measured_ops = 0;
    while (ns_now(CLOCK_MONOTONIC) - wall_start < measure_ns || measured_ops == 0) {
        for (size_t i = 0; i < size; ++i) {
            checksum = (checksum << 1 | checksum >> 63) ^ lookup_decode(&mmdb, workload_get(&workload, i));
            measured_ops++;
        }
    }
    uint64_t wall_ns = ns_now(CLOCK_MONOTONIC) - wall_start;
    uint64_t cpu_ns = ns_now(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
    uint64_t allocations = allocation_calls;
    uint64_t bytes = allocated_bytes;
    uint64_t rss_after = current_rss_bytes();
    uint64_t peak_rss = peak_rss_bytes();
    sink ^= checksum;

    print_lookup_result("lookup", 1, family, pattern, size, warmup, measured_ops, samples, sample_count, wall_ns, cpu_ns,
                        allocations, bytes, rss_before, rss_after, peak_rss, first_ns, checksum,
                        dataset);

    free(samples);
    MMDB_close(&mmdb);
    workload_free(&workload);
    return 0;
}

typedef struct {
    MMDB_s *mmdb;
    const workload_t *workload;
    size_t start_idx;
    size_t end_idx;
    pthread_barrier_t *barrier;
    uint64_t checksum;
    uint64_t calls, bytes;
    uint64_t deadline_ns;
    size_t operations;
    int timed;
    uint64_t *samples;
    size_t sample_count, workload_size;
} thread_lookup_arg_t;

static void *worker_lookup_func(void *ptr) {
    thread_lookup_arg_t *arg = (thread_lookup_arg_t *)ptr;
    allocation_calls = allocated_bytes = 0;
    if (arg->barrier) {
        pthread_barrier_wait(arg->barrier);
        pthread_barrier_wait(arg->barrier);
    }
    uint64_t checksum = 0;
    size_t count = 0;
    if (arg->timed) {
        size_t span = arg->end_idx > arg->start_idx ? arg->end_idx - arg->start_idx : 1;
        while (ns_now(CLOCK_MONOTONIC) < arg->deadline_ns || count == 0) {
            size_t i = arg->start_idx + count % span;
            uint64_t val = lookup_decode(arg->mmdb, workload_get(arg->workload, i % arg->workload_size));
            checksum = (checksum << 1 | checksum >> 63) ^ val;
            count++;
        }
    } else for (size_t i = arg->start_idx; i < arg->end_idx; ++i) {
        size_t index = arg->samples ? i * arg->workload_size / arg->sample_count : i;
        uint64_t start = arg->samples ? ns_now(CLOCK_MONOTONIC) : 0;
        uint64_t val = lookup_decode(arg->mmdb, workload_get(arg->workload, index));
        if (arg->samples) arg->samples[i] = ns_now(CLOCK_MONOTONIC) - start;
        checksum = (checksum << 1 | checksum >> 63) ^ val;
        count++;
    }
    arg->checksum = checksum;
    arg->operations = count;
    arg->calls = allocation_calls;
    arg->bytes = allocated_bytes;
    if (arg->barrier) pthread_barrier_wait(arg->barrier);
    return NULL;
}

static int benchmark_lookup_concurrent(const char *dataset) {
    const char *workload_dir = getenv("BENCH_WORKLOAD_DIR");
    const char *family = getenv("BENCH_FAMILY");
    const char *pattern = getenv("BENCH_PATTERN");
    if (!workload_dir || !family || !pattern) return 2;
    size_t size = env_size("BENCH_WORKLOAD_SIZE", 100000);
    uint64_t warmup_ns = benchmark_duration_ns("BENCH_WARMUP_SECS");
    uint64_t measure_ns = benchmark_duration_ns("BENCH_MEASURE_SECS");
    size_t sample_max = env_size("BENCH_LATENCY_SAMPLES_MAX", 100000);
    size_t sample_count = size < sample_max ? size : sample_max;

    long nproc = sysconf(_SC_NPROCESSORS_ONLN);
    if (nproc < 1) nproc = 1;
    size_t threads = env_size("BENCH_THREADS", (size_t)nproc);
    if (threads < 1) threads = 1;

    workload_t workload;
    if (load_workload(&workload, workload_dir, family, pattern, size) != 0) return 3;

    MMDB_s mmdb;
    int status = MMDB_open(dataset, MMDB_MODE_MMAP, &mmdb);
    if (status != MMDB_SUCCESS) {
        workload_free(&workload);
        return 4;
    }

    uint64_t first_start = ns_now(CLOCK_MONOTONIC);
    uint64_t first_value = lookup_decode(&mmdb, workload_get(&workload, 0));
    uint64_t first_ns = ns_now(CLOCK_MONOTONIC) - first_start;
    /* first_value may be 0 if first IP is not present */

    pthread_t *thread_handles = calloc(threads, sizeof(pthread_t));
    thread_lookup_arg_t *thread_args = calloc(threads, sizeof(thread_lookup_arg_t));
    if (!thread_handles || !thread_args) {
        free(thread_handles);
        free(thread_args);
        MMDB_close(&mmdb);
        workload_free(&workload);
        return 6;
    }

    uint64_t warmup_deadline = ns_now(CLOCK_MONOTONIC) + warmup_ns;
    for (size_t t = 0; t < threads; ++t) {
        thread_args[t].mmdb = &mmdb;
        thread_args[t].workload = &workload;
        thread_args[t].start_idx = t * size / threads;
        thread_args[t].end_idx = (t + 1) * size / threads;
        thread_args[t].workload_size = size;
        thread_args[t].deadline_ns = warmup_deadline;
        thread_args[t].timed = 1;
        if (thread_args[t].end_idx > size) thread_args[t].end_idx = size;
        thread_args[t].barrier = NULL;
        if (pthread_create(&thread_handles[t], NULL, worker_lookup_func, &thread_args[t])) abort();
    }
    for (size_t t = 0; t < threads; ++t) {
        pthread_join(thread_handles[t], NULL);
    }
    size_t warmup = 0;
    for (size_t t = 0; t < threads; ++t) warmup += thread_args[t].operations;

    uint64_t *samples = calloc(sample_count, sizeof(*samples));
    if (!samples) {
        free(thread_handles);
        free(thread_args);
        MMDB_close(&mmdb);
        workload_free(&workload);
        return 6;
    }
    pthread_barrier_t barrier;
    pthread_barrier_init(&barrier, NULL, (unsigned)threads + 1);
    size_t sample_chunk = (sample_count + threads - 1) / threads;
    for (size_t t = 0; t < threads; ++t) {
        thread_args[t].start_idx = t * sample_chunk;
        thread_args[t].end_idx = (t + 1) * sample_chunk;
        if (thread_args[t].end_idx > sample_count) thread_args[t].end_idx = sample_count;
        thread_args[t].samples = samples;
        thread_args[t].sample_count = sample_count;
        thread_args[t].workload_size = size;
        thread_args[t].barrier = &barrier;
        thread_args[t].timed = 0;
        if (pthread_create(&thread_handles[t], NULL, worker_lookup_func, &thread_args[t])) abort();
    }
    pthread_barrier_wait(&barrier);
    pthread_barrier_wait(&barrier);
    pthread_barrier_wait(&barrier);
    for (size_t t = 0; t < threads; ++t) pthread_join(thread_handles[t], NULL);

    size_t chunk_size = (size + threads - 1) / threads;
    for (size_t t = 0; t < threads; ++t) {
        thread_args[t].mmdb = &mmdb;
        thread_args[t].workload = &workload;
        thread_args[t].start_idx = t * chunk_size;
        thread_args[t].end_idx = (t + 1) * chunk_size;
        if (thread_args[t].start_idx > size) thread_args[t].start_idx = size;
        if (thread_args[t].end_idx > size) thread_args[t].end_idx = size;
        thread_args[t].barrier = &barrier;
        thread_args[t].samples = NULL;
        thread_args[t].timed = 1;
        thread_args[t].deadline_ns = ns_now(CLOCK_MONOTONIC) + measure_ns + UINT64_C(10000000);
        if (pthread_create(&thread_handles[t], NULL, worker_lookup_func, &thread_args[t])) abort();
    }

    uint64_t rss_before = current_rss_bytes();
    allocation_calls = 0;
    allocated_bytes = 0;

    pthread_barrier_wait(&barrier);
    uint64_t cpu_start = ns_now(CLOCK_PROCESS_CPUTIME_ID);
    uint64_t wall_start = ns_now(CLOCK_MONOTONIC);
    pthread_barrier_wait(&barrier);
    pthread_barrier_wait(&barrier);
    uint64_t wall_ns = ns_now(CLOCK_MONOTONIC) - wall_start;
    uint64_t cpu_ns = ns_now(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
    uint64_t checksum = 0, allocations = 0, bytes = 0;
    size_t measured_ops = 0;
    for (size_t t = 0; t < threads; ++t) {
        pthread_join(thread_handles[t], NULL);
        checksum = (checksum << 1 | checksum >> 63) ^ thread_args[t].checksum;
        allocations += thread_args[t].calls;
        bytes += thread_args[t].bytes;
        measured_ops += thread_args[t].operations;
    }
    uint64_t rss_after = current_rss_bytes();
    uint64_t peak_rss = peak_rss_bytes();
    sink ^= checksum;

    print_lookup_result("lookup_concurrent", threads, family, pattern, size, warmup, measured_ops, samples, sample_count,
                        wall_ns, cpu_ns, allocations, bytes, rss_before, rss_after, peak_rss, first_ns, checksum,
                        dataset);

    pthread_barrier_destroy(&barrier);
    free(samples);
    free(thread_handles);
    free(thread_args);
    MMDB_close(&mmdb);
    workload_free(&workload);
    return 0;
}

static int benchmark_open_file(const char *dataset) {
    size_t iterations = env_size("BENCH_LATENCY_SAMPLES_MAX", 100000);
    uint64_t warmup_ns = benchmark_duration_ns("BENCH_WARMUP_SECS");
    uint64_t measure_ns = benchmark_duration_ns("BENCH_MEASURE_SECS");

    uint64_t first_start = ns_now(CLOCK_MONOTONIC);
    MMDB_s first;
    if (MMDB_open(dataset, 0, &first) != MMDB_SUCCESS) return 4;
    MMDB_close(&first);
    uint64_t first_ns = ns_now(CLOCK_MONOTONIC) - first_start;

    uint64_t warm_start = ns_now(CLOCK_MONOTONIC);
    size_t warmup = 0;
    while (ns_now(CLOCK_MONOTONIC) - warm_start < warmup_ns) {
        MMDB_s temp;
        if (MMDB_open(dataset, 0, &temp) != MMDB_SUCCESS) return 4;
        MMDB_close(&temp);
        warmup++;
    }

    uint64_t *samples = calloc(iterations, sizeof(*samples));
    if (!samples) return 6;
    for (size_t i = 0; i < iterations; ++i) {
        uint64_t start = ns_now(CLOCK_MONOTONIC);
        MMDB_s temp;
        if (MMDB_open(dataset, 0, &temp) != MMDB_SUCCESS) {
            free(samples);
            return 4;
        }
        MMDB_close(&temp);
        samples[i] = ns_now(CLOCK_MONOTONIC) - start;
    }

    uint64_t rss_before = current_rss_bytes();
    allocation_calls = 0;
    allocated_bytes = 0;
    uint64_t cpu_start = ns_now(CLOCK_PROCESS_CPUTIME_ID);
    uint64_t wall_start = ns_now(CLOCK_MONOTONIC);
    uint64_t checksum = 0;
    size_t measured_ops = 0;
    while (ns_now(CLOCK_MONOTONIC) - wall_start < measure_ns || measured_ops == 0) {
        MMDB_s temp;
        int status = MMDB_open(dataset, 0, &temp);
        if (status != MMDB_SUCCESS) {
            free(samples);
            return 4;
        }
        checksum ^= (uint64_t)temp.metadata.node_count;
        MMDB_close(&temp);
        measured_ops++;
    }
    uint64_t wall_ns = ns_now(CLOCK_MONOTONIC) - wall_start;
    uint64_t cpu_ns = ns_now(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
    uint64_t allocations = allocation_calls;
    uint64_t bytes = allocated_bytes;
    uint64_t rss_after = current_rss_bytes();
    uint64_t peak_rss = peak_rss_bytes();
    sink ^= checksum;

    /* Open scenarios use null family/pattern. */
    stats_t stats = summarize(samples, iterations);
    double throughput = wall_ns ? (double)measured_ops * 1e9 / (double)wall_ns : 0.0;
    double cpu_pct = wall_ns ? (double)cpu_ns * 100.0 / (double)wall_ns : 0.0;
    const char *revision = getenv("BENCH_IMPLEMENTATION_REVISION");
    if (!revision) revision = "unknown";
    printf(
        "{\"schema_version\":2,\"implementation\":\"libmaxminddb\",\"version\":\"%s\","
        "\"revision\":\"%s\",\"scenario\":\"open_file\",\"operation\":\"open\","
        "\"family\":null,\"pattern\":null,\"workload_size\":%zu,\"warmup_ops\":%zu,\"measurement_operations\":%zu,"
        "\"latency_sample_count\":%zu,\"mean_ns\":%.3f,\"median_ns\":%.3f,"
        "\"min_ns\":%llu,\"max_ns\":%llu,\"p50_ns\":%llu,\"p95_ns\":%llu,"
        "\"p99_ns\":%llu,\"variance_ns2\":%.3f,\"stddev_ns\":%.3f,"
        "\"throughput_ops_s\":%.3f,\"wall_time_ns\":%llu,\"cpu_time_ns\":%llu,"
        "\"cpu_utilization_pct\":%.3f,\"allocation_calls\":%llu,\"allocations_per_op\":%.9f,"
        "\"allocated_bytes\":%llu,\"allocated_bytes_per_op\":%.3f,"
        "\"rss_before_bytes\":%llu,\"rss_after_bytes\":%llu,\"rss_delta_bytes\":%lld,"
        "\"peak_rss_bytes\":%llu,\"first_operation_ns\":%llu,\"timer_overhead_ns\":%.3f,"
        "\"dataset_bytes\":%llu,\"checksum\":%llu}\n",
        MMDB_lib_version(), revision, iterations, warmup, measured_ops, iterations, stats.mean, stats.median,
        (unsigned long long)stats.min, (unsigned long long)stats.max,
        (unsigned long long)stats.p50, (unsigned long long)stats.p95,
        (unsigned long long)stats.p99, stats.variance, stats.stddev, throughput,
        (unsigned long long)wall_ns, (unsigned long long)cpu_ns, cpu_pct,
        (unsigned long long)allocations, (double)allocations / (double)measured_ops,
        (unsigned long long)bytes, (double)bytes / (double)measured_ops,
        (unsigned long long)rss_before, (unsigned long long)rss_after,
        (long long)rss_after - (long long)rss_before, (unsigned long long)peak_rss,
        (unsigned long long)first_ns, timer_overhead_ns(),
        (unsigned long long)dataset_size(dataset), (unsigned long long)checksum);

    free(samples);
    return 0;
}

static int benchmark_open_mmap(const char *dataset) {
    size_t iterations = env_size("BENCH_LATENCY_SAMPLES_MAX", 100000);
    uint64_t warmup_ns = benchmark_duration_ns("BENCH_WARMUP_SECS");
    uint64_t measure_ns = benchmark_duration_ns("BENCH_MEASURE_SECS");

    uint64_t first_start = ns_now(CLOCK_MONOTONIC);
    MMDB_s first;
    if (MMDB_open(dataset, MMDB_MODE_MMAP, &first) != MMDB_SUCCESS) return 4;
    MMDB_close(&first);
    uint64_t first_ns = ns_now(CLOCK_MONOTONIC) - first_start;

    uint64_t warm_start = ns_now(CLOCK_MONOTONIC);
    size_t warmup = 0;
    while (ns_now(CLOCK_MONOTONIC) - warm_start < warmup_ns) {
        MMDB_s temp;
        if (MMDB_open(dataset, MMDB_MODE_MMAP, &temp) != MMDB_SUCCESS) return 4;
        MMDB_close(&temp);
        warmup++;
    }

    uint64_t *samples = calloc(iterations, sizeof(*samples));
    if (!samples) return 6;
    for (size_t i = 0; i < iterations; ++i) {
        uint64_t start = ns_now(CLOCK_MONOTONIC);
        MMDB_s temp;
        if (MMDB_open(dataset, MMDB_MODE_MMAP, &temp) != MMDB_SUCCESS) {
            free(samples);
            return 4;
        }
        MMDB_close(&temp);
        samples[i] = ns_now(CLOCK_MONOTONIC) - start;
    }

    uint64_t rss_before = current_rss_bytes();
    allocation_calls = 0;
    allocated_bytes = 0;
    uint64_t cpu_start = ns_now(CLOCK_PROCESS_CPUTIME_ID);
    uint64_t wall_start = ns_now(CLOCK_MONOTONIC);
    uint64_t checksum = 0;
    size_t measured_ops = 0;
    while (ns_now(CLOCK_MONOTONIC) - wall_start < measure_ns || measured_ops == 0) {
        MMDB_s temp;
        int status = MMDB_open(dataset, MMDB_MODE_MMAP, &temp);
        if (status != MMDB_SUCCESS) {
            free(samples);
            return 4;
        }
        checksum ^= (uint64_t)temp.metadata.node_count;
        MMDB_close(&temp);
        measured_ops++;
    }
    uint64_t wall_ns = ns_now(CLOCK_MONOTONIC) - wall_start;
    uint64_t cpu_ns = ns_now(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
    uint64_t allocations = allocation_calls;
    uint64_t bytes = allocated_bytes;
    uint64_t rss_after = current_rss_bytes();
    uint64_t peak_rss = peak_rss_bytes();
    sink ^= checksum;

    /* Open scenarios use null family/pattern. */
    stats_t stats = summarize(samples, iterations);
    double throughput = wall_ns ? (double)measured_ops * 1e9 / (double)wall_ns : 0.0;
    double cpu_pct = wall_ns ? (double)cpu_ns * 100.0 / (double)wall_ns : 0.0;
    const char *revision = getenv("BENCH_IMPLEMENTATION_REVISION");
    if (!revision) revision = "unknown";
    printf(
        "{\"schema_version\":2,\"implementation\":\"libmaxminddb\",\"version\":\"%s\","
        "\"revision\":\"%s\",\"scenario\":\"open_mmap\",\"operation\":\"open\","
        "\"family\":null,\"pattern\":null,\"workload_size\":%zu,\"warmup_ops\":%zu,\"measurement_operations\":%zu,"
        "\"latency_sample_count\":%zu,\"mean_ns\":%.3f,\"median_ns\":%.3f,"
        "\"min_ns\":%llu,\"max_ns\":%llu,\"p50_ns\":%llu,\"p95_ns\":%llu,"
        "\"p99_ns\":%llu,\"variance_ns2\":%.3f,\"stddev_ns\":%.3f,"
        "\"throughput_ops_s\":%.3f,\"wall_time_ns\":%llu,\"cpu_time_ns\":%llu,"
        "\"cpu_utilization_pct\":%.3f,\"allocation_calls\":%llu,\"allocations_per_op\":%.9f,"
        "\"allocated_bytes\":%llu,\"allocated_bytes_per_op\":%.3f,"
        "\"rss_before_bytes\":%llu,\"rss_after_bytes\":%llu,\"rss_delta_bytes\":%lld,"
        "\"peak_rss_bytes\":%llu,\"first_operation_ns\":%llu,\"timer_overhead_ns\":%.3f,"
        "\"dataset_bytes\":%llu,\"checksum\":%llu}\n",
        MMDB_lib_version(), revision, iterations, warmup, measured_ops, iterations, stats.mean, stats.median,
        (unsigned long long)stats.min, (unsigned long long)stats.max,
        (unsigned long long)stats.p50, (unsigned long long)stats.p95,
        (unsigned long long)stats.p99, stats.variance, stats.stddev, throughput,
        (unsigned long long)wall_ns, (unsigned long long)cpu_ns, cpu_pct,
        (unsigned long long)allocations, (double)allocations / (double)measured_ops,
        (unsigned long long)bytes, (double)bytes / (double)measured_ops,
        (unsigned long long)rss_before, (unsigned long long)rss_after,
        (long long)rss_after - (long long)rss_before, (unsigned long long)peak_rss,
        (unsigned long long)first_ns, timer_overhead_ns(),
        (unsigned long long)dataset_size(dataset), (unsigned long long)checksum);

    free(samples);
    return 0;
}

/* memory-rss-v2 uses the same prepared binary IPv4 query file in every
 * harness. VmHWM is reset after open; no sampled maximum or inherited rusage. */
static int memory_rss_snapshot(uint64_t *rss, uint64_t *peak) {
    FILE *file = fopen("/proc/self/status", "r");
    if (!file) return -1;
    char line[256];
    unsigned long long value;
    *rss = *peak = 0;
    while (fgets(line, sizeof(line), file)) {
        if (sscanf(line, "VmRSS: %llu kB", &value) == 1) *rss = value * 1024ULL;
        if (sscanf(line, "VmHWM: %llu kB", &value) == 1) *peak = value * 1024ULL;
    }
    fclose(file);
    return (*rss && *peak) ? 0 : -1;
}

static int benchmark_memory(const char *dataset) {
    const size_t lookups = env_size("BENCH_MEM_LOOKUPS", 1000000);
    const size_t warmup = env_size_allow_zero("BENCH_WARMUP_OPS", 1000);
    const char *dir = getenv("BENCH_WORKLOAD_DIR");
    if (!dir || lookups != 1000000 || warmup != 1000) {
        fprintf(stderr, "memory-rss-v2 requires 1M binary queries and 1000 warmup lookups\n");
        return 4;
    }
    char *path = malloc(strlen(dir) + 32);
    if (!path) return 4;
    sprintf(path, "%s/memory-queries.bin", dir);
    if (dataset_size(path) != lookups * 4) { fprintf(stderr, "Invalid memory workload length\n"); free(path); return 4; }
    FILE *input = fopen(path, "rb");
    free(path);
    unsigned char *queries = malloc(lookups * 4);
    if (!input || !queries) { if (input) fclose(input); free(queries); return 4; }
    size_t loaded = fread(queries, 4, lookups, input);
    fclose(input);
    if (loaded != lookups) { fprintf(stderr, "Cannot load memory workload\n"); free(queries); return 4; }

    uint64_t rss_before, rss_after_open, rss_after, rss_peak, unused;
    if (memory_rss_snapshot(&rss_before, &unused)) { fprintf(stderr, "RSS unavailable: /proc/self/status\n"); free(queries); return 4; }
    MMDB_s mmdb;
    int status = MMDB_open(dataset, MMDB_MODE_MMAP, &mmdb);
    if (status != MMDB_SUCCESS) { fprintf(stderr, "MMDB_open: %s\n", MMDB_strerror(status)); free(queries); return 4; }
    if (memory_rss_snapshot(&rss_after_open, &unused)) { MMDB_close(&mmdb); free(queries); return 4; }
    FILE *reset = fopen("/proc/self/clear_refs", "w");
    int reset_ok = 0;
    if (reset) {
        int written = fputs("5\n", reset) >= 0;
        reset_ok = fclose(reset) == 0 && written;
    }
    uint64_t checksum = 0;
    size_t hits = 0, misses = 0;
    struct sockaddr_in sa = {0};
    sa.sin_family = AF_INET;
    for (size_t i = 0; i < warmup + lookups; ++i) {
        size_t index = i < warmup ? i : i - warmup;
        const unsigned char *q = queries + 4 * index;
        uint32_t raw = ((uint32_t)q[0] << 24) | ((uint32_t)q[1] << 16) | ((uint32_t)q[2] << 8) | q[3];
        sa.sin_addr.s_addr = htonl(raw);
        int error = MMDB_SUCCESS;
        MMDB_lookup_result_s result = MMDB_lookup_sockaddr(&mmdb, (const struct sockaddr *)&sa, &error);
        if (error != MMDB_SUCCESS || !!result.found_entry != !(raw & 1)) {
            fprintf(stderr, "Unexpected memory lookup result for %08x: %s\n", raw, MMDB_strerror(error));
            MMDB_close(&mmdb); free(queries); return 4;
        }
        if (result.found_entry) {
            MMDB_entry_data_list_s *list = NULL;
            error = MMDB_get_entry_data_list(&result.entry, &list);
            if (error != MMDB_SUCCESS || !list) {
                fprintf(stderr, "Memory lookup decode failed: %s\n", MMDB_strerror(error));
                if (list) MMDB_free_entry_data_list(list);
                MMDB_close(&mmdb); free(queries); return 4;
            }
            sink ^= (uint64_t)(uintptr_t)list;
            MMDB_free_entry_data_list(list);
        }
        if (i >= warmup) {
            hits += !!result.found_entry;
            misses += !result.found_entry;
            checksum = (checksum << 1 | checksum >> 63) ^ raw ^ !!result.found_entry;
        }
    }
    sink ^= checksum;
    if (memory_rss_snapshot(&rss_after, &rss_peak)) { MMDB_close(&mmdb); free(queries); return 4; }
    char peak_json[32];
    if (reset_ok) snprintf(peak_json, sizeof(peak_json), "%llu", (unsigned long long)rss_peak);
    else strcpy(peak_json, "null");
    const char *revision = getenv("BENCH_IMPLEMENTATION_REVISION");
    if (!revision) revision = "unknown";
    printf(
        "{\"schema_version\":2,\"implementation\":\"libmaxminddb\",\"version\":\"%s\","
        "\"revision\":\"%s\",\"scenario\":\"memory_rss\",\"operation\":\"lookup_decode\","
        "\"threads\":1,\"family\":\"ipv4\",\"pattern\":\"mixed_50_50\",\"workload_size\":%zu,"
        "\"lookups\":%zu,\"warmup_ops\":%zu,\"hits\":%zu,\"misses\":%zu,\"pid\":%ld,\"auxiliary_bytes\":%zu,"
        "\"rss_before_open_bytes\":%llu,\"rss_after_open_bytes\":%llu,\"rss_peak_bytes\":%s,\"rss_after_bytes\":%llu,"
        "\"opening_mode\":\"mmap\",\"measurement_protocol\":\"memory-rss-v2\","
        "\"rss_method\":\"linux-vmrss-vmhwm-reset-after-open\",\"peak_unavailable_reason\":%s,"
        "\"checksum\":%llu,\"dataset_bytes\":%llu}\n",
        MMDB_lib_version(), revision, lookups, lookups, warmup, hits, misses, (long)getpid(), lookups * 4,
        (unsigned long long)rss_before, (unsigned long long)rss_after_open, peak_json, (unsigned long long)rss_after,
        reset_ok ? "null" : "\"Cannot reset VmHWM after open\"",
        (unsigned long long)checksum, (unsigned long long)dataset_size(dataset));
    MMDB_close(&mmdb);
    free(queries);
    return 0;
}

int main(void) {
    const char *dataset = getenv("BENCH_MMDB");
    const char *scenario = getenv("BENCH_SCENARIO");
    if (!dataset || !scenario) return 2;
    if (strcmp(scenario, "lookup") == 0) return benchmark_lookup(dataset);
    if (strcmp(scenario, "lookup_concurrent") == 0) return benchmark_lookup_concurrent(dataset);
    if (strcmp(scenario, "memory_rss") == 0) return benchmark_memory(dataset);
    if (strcmp(scenario, "open_mmap") == 0) return benchmark_open_mmap(dataset);
    if (strcmp(scenario, "open_file") == 0) return benchmark_open_file(dataset);
    /* libmaxminddb does not expose an in-memory-buffer constructor or read-into-Vec mode. */
    if (strcmp(scenario, "open_buffer") == 0) return 64;
    return 65;
}
