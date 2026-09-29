package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"math"
	"net/netip"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/oschwald/maxminddb-golang/v2"
)

const Version = "v2.6.0"

// GeoIP2-City structure matching the C and Rust benchmarks
type Names struct {
	En string `maxminddb:"en"`
}

type Continent struct {
	Code      string `maxminddb:"code"`
	GeonameID uint32 `maxminddb:"geoname_id"`
	Names     Names  `maxminddb:"names"`
}

type Country struct {
	GeonameID uint32 `maxminddb:"geoname_id"`
	ISOCode   string `maxminddb:"iso_code"`
	Names     Names  `maxminddb:"names"`
}

type Subdivision struct {
	GeonameID uint32 `maxminddb:"geoname_id"`
	ISOCode   string `maxminddb:"iso_code"`
	Names     Names  `maxminddb:"names"`
}

type City struct {
	GeonameID uint32 `maxminddb:"geoname_id"`
	Names     Names  `maxminddb:"names"`
}

type Location struct {
	AccuracyRadius uint16  `maxminddb:"accuracy_radius"`
	Latitude       float64 `maxminddb:"latitude"`
	Longitude      float64 `maxminddb:"longitude"`
	TimeZone       string  `maxminddb:"time_zone"`
}

type CityRecord struct {
	Continent         Continent     `maxminddb:"continent"`
	Country           Country       `maxminddb:"country"`
	Subdivisions      []Subdivision `maxminddb:"subdivisions"`
	City              City          `maxminddb:"city"`
	Location          Location      `maxminddb:"location"`
	RegisteredCountry Country       `maxminddb:"registered_country"`
}

type Config struct {
	Dataset           string
	WorkloadDir       string
	Scenario          string
	Family            string
	Pattern           string
	WorkloadSize      int
	WarmupOps         int
	LatencySamplesMax int
	OpenIterations    int
	Revision          string
	Threads           int
}

func parseEnvInt(name string, defVal int) int {
	val := os.Getenv(name)
	if val == "" {
		return defVal
	}
	n, err := strconv.Atoi(val)
	if err != nil || n <= 0 {
		return defVal
	}
	return n
}

func loadConfig() (*Config, error) {
	scenario := os.Getenv("BENCH_SCENARIO")
	if scenario == "" {
		scenario = "lookup"
	}
	dataset := os.Getenv("BENCH_MMDB")
	if dataset == "" {
		return nil, fmt.Errorf("BENCH_MMDB is required")
	}
	workloadDir := os.Getenv("BENCH_WORKLOAD_DIR")
	if workloadDir == "" && scenario != "open_file" && scenario != "open_buffer" && scenario != "open_mmap" {
		return nil, fmt.Errorf("BENCH_WORKLOAD_DIR is required")
	}

	family := strings.ToLower(os.Getenv("BENCH_FAMILY"))
	pattern := strings.ToLower(os.Getenv("BENCH_PATTERN"))

	threads := parseEnvInt("BENCH_THREADS", runtime.NumCPU())
	if threads <= 0 {
		threads = 1
	}

	cfg := &Config{
		Dataset:           dataset,
		WorkloadDir:       workloadDir,
		Scenario:          scenario,
		Family:            family,
		Pattern:           pattern,
		WorkloadSize:      parseEnvInt("BENCH_WORKLOAD_SIZE", 100000),
		WarmupOps:         parseEnvInt("BENCH_WARMUP_OPS", 10000),
		LatencySamplesMax: parseEnvInt("BENCH_LATENCY_SAMPLES_MAX", 100000),
		OpenIterations:    parseEnvInt("BENCH_OPEN_ITERS", 200),
		Revision:          os.Getenv("BENCH_IMPLEMENTATION_REVISION"),
		Threads:           threads,
	}
	if cfg.Revision == "" {
		cfg.Revision = Version
	}
	return cfg, nil
}

func loadWorkload(cfg *Config) ([]netip.Addr, error) {
	if cfg.Pattern == "hot" {
		defaultIP := "81.2.69.160"
		envName := "BENCH_HOT_IPV4"
		if cfg.Family == "ipv6" {
			defaultIP = "2001:db8:123::1"
			envName = "BENCH_HOT_IPV6"
		}
		ipStr := os.Getenv(envName)
		if ipStr == "" {
			ipStr = defaultIP
		}
		ip, err := netip.ParseAddr(ipStr)
		if err != nil {
			return nil, err
		}
		ips := make([]netip.Addr, cfg.WorkloadSize)
		for i := range ips {
			ips[i] = ip
		}
		return ips, nil
	}

	filename := fmt.Sprintf("%s-%s.txt", cfg.Family, cfg.Pattern)
	path := filepath.Join(cfg.WorkloadDir, filename)
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()

	var ips []netip.Addr
	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		ip, err := netip.ParseAddr(line)
		if err != nil {
			return nil, err
		}
		ips = append(ips, ip)
		if len(ips) >= cfg.WorkloadSize {
			break
		}
	}
	if err := scanner.Err(); err != nil {
		return nil, err
	}
	if len(ips) != cfg.WorkloadSize {
		return nil, fmt.Errorf("%s contains %d IPs but %d were requested", path, len(ips), cfg.WorkloadSize)
	}
	return ips, nil
}

type BenchResult struct {
	SchemaVersion       int     `json:"schema_version"`
	Implementation      string  `json:"implementation"`
	Language            string  `json:"language"`
	Version             string  `json:"version"`
	Revision            string  `json:"revision"`
	Scenario            string  `json:"scenario"`
	Operation           string  `json:"operation"`
	Threads             int     `json:"threads"`
	Family              *string `json:"family,omitempty"`
	Pattern             *string `json:"pattern,omitempty"`
	WorkloadSize        int     `json:"workload_size"`
	WarmupOps           int     `json:"warmup_ops"`
	MeasurementOps      int     `json:"measurement_operations"`
	WarmupDurationNs    uint64  `json:"warmup_duration_ns"`
	MeasurementWindowNs uint64  `json:"measurement_window_ns"`
	LatencySampleCount  int     `json:"latency_sample_count"`
	MeanNs              float64 `json:"mean_ns"`
	MedianNs            float64 `json:"median_ns"`
	MinNs               uint64  `json:"min_ns"`
	MaxNs               uint64  `json:"max_ns"`
	P50Ns               uint64  `json:"p50_ns"`
	P95Ns               uint64  `json:"p95_ns"`
	P99Ns               uint64  `json:"p99_ns"`
	VarianceNs2         float64 `json:"variance_ns2"`
	StddevNs            float64 `json:"stddev_ns"`
	ThroughputOpsS      float64 `json:"throughput_ops_s"`
	WallTimeNs          uint64  `json:"wall_time_ns"`
	CpuTimeNs           uint64  `json:"cpu_time_ns"`
	CpuUtilizationPct   float64 `json:"cpu_utilization_pct"`
	AllocationCalls     uint64  `json:"allocation_calls"`
	AllocationsPerOp    float64 `json:"allocations_per_op"`
	AllocatedBytes      uint64  `json:"allocated_bytes"`
	AllocatedBytesPerOp float64 `json:"allocated_bytes_per_op"`
	RssBeforeBytes      uint64  `json:"rss_before_bytes"`
	RssAfterBytes       uint64  `json:"rss_after_bytes"`
	RssDeltaBytes       int64   `json:"rss_delta_bytes"`
	PeakRssBytes        uint64  `json:"peak_rss_bytes"`
	FirstOperationNs    uint64  `json:"first_operation_ns"`
	TimerOverheadNs     float64 `json:"timer_overhead_ns"`
	DatasetBytes        uint64  `json:"dataset_bytes"`
	Checksum            uint64  `json:"checksum"`
}

type Stats struct {
	Mean     float64
	Median   float64
	Min      uint64
	Max      uint64
	P50      uint64
	P95      uint64
	P99      uint64
	Variance float64
	Stddev   float64
}

func percentile(sorted []uint64, q float64) uint64 {
	if len(sorted) == 0 {
		return 0
	}
	idx := int(math.Round(float64(len(sorted)-1) * q))
	if idx < 0 {
		idx = 0
	}
	if idx >= len(sorted) {
		idx = len(sorted) - 1
	}
	return sorted[idx]
}

func summarize(samples []uint64) Stats {
	if len(samples) == 0 {
		return Stats{}
	}
	sort.Slice(samples, func(i, j int) bool { return samples[i] < samples[j] })
	n := float64(len(samples))
	var sum uint64
	for _, v := range samples {
		sum += v
	}
	mean := float64(sum) / n
	var varSum float64
	for _, v := range samples {
		d := float64(v) - mean
		varSum += d * d
	}
	variance := varSum / n
	return Stats{
		Mean:     mean,
		Median:   float64(percentile(samples, 0.50)),
		Min:      samples[0],
		Max:      samples[len(samples)-1],
		P50:      percentile(samples, 0.50),
		P95:      percentile(samples, 0.95),
		P99:      percentile(samples, 0.99),
		Variance: variance,
		Stddev:   math.Sqrt(variance),
	}
}

func currentRssBytes() uint64 {
	data, err := os.ReadFile("/proc/self/statm")
	if err == nil {
		fields := strings.Fields(string(data))
		if len(fields) >= 2 {
			if pages, err := strconv.ParseUint(fields[1], 10, 64); err == nil {
				pageSize := uint64(os.Getpagesize())
				return pages * pageSize
			}
		}
	}
	return peakRssBytes()
}

func peakRssBytes() uint64 {
	var usage syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &usage); err == nil {
		return uint64(usage.Maxrss) * 1024
	}
	return 0
}

func processCpuNs() uint64 {
	var usage syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &usage); err == nil {
		return uint64(usage.Utime.Sec+usage.Stime.Sec)*1000000000 +
			uint64(usage.Utime.Usec+usage.Stime.Usec)*1000
	}
	return 0
}

func measureTimerOverhead() float64 {
	const iters = 10000
	t0 := time.Now()
	for i := 0; i < iters; i++ {
		_ = time.Now()
	}
	elapsed := time.Since(t0).Nanoseconds()
	return float64(elapsed) / float64(iters)
}

func checksumRecord(record *CityRecord) uint64 {
	val := uint64(record.City.GeonameID)
	val ^= uint64(record.Country.GeonameID) << 17
	if len(record.Country.ISOCode) > 0 {
		val ^= uint64(record.Country.ISOCode[0]) << 32
	}
	return val
}

func benchOpenFromFile(cfg *Config) error {
	// For maxminddb-golang, open_file is implemented by reading the file
	// into memory and using OpenBytes, similar to open_buffer
	fileBytes, err := os.ReadFile(cfg.Dataset)
	if err != nil {
		return err
	}

	fi, err := os.Stat(cfg.Dataset)
	if err != nil {
		return err
	}
	datasetBytes := uint64(fi.Size())
	timerOverhead := measureTimerOverhead()

	var firstOpNs uint64
	var checksum uint64
	warmStart := time.Now()
	warmupOps := 0
	for time.Since(warmStart) < time.Second {
		db, openErr := maxminddb.OpenBytes(fileBytes)
		if openErr != nil {
			return openErr
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		warmupOps++
	}
	samples := make([]uint64, 0, cfg.OpenIterations)

	for i := 0; i < cfg.OpenIterations; i++ {
		t0 := time.Now()
		db, openErr := maxminddb.OpenBytes(fileBytes)
		if openErr != nil {
			return openErr
		}
		dt := uint64(time.Since(t0).Nanoseconds())
		if i == 0 {
			firstOpNs = dt
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		samples = append(samples, dt)
	}
	rssBefore := currentRssBytes()
	var mBefore, mAfter runtime.MemStats
	runtime.ReadMemStats(&mBefore)
	cpuBefore := processCpuNs()
	wallStart := time.Now()
	measurementOps := 0
	for time.Since(wallStart) < 2*time.Second || measurementOps == 0 {
		db, openErr := maxminddb.OpenBytes(fileBytes)
		if openErr != nil {
			return openErr
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		measurementOps++
	}

	wallNs := uint64(time.Since(wallStart).Nanoseconds())
	cpuNs := processCpuNs() - cpuBefore
	runtime.ReadMemStats(&mAfter)
	rssAfter := currentRssBytes()
	peakRss := peakRssBytes()

	stats := summarize(samples)
	throughput := float64(measurementOps) * 1e9 / float64(wallNs)
	cpuUtil := 0.0
	if wallNs > 0 {
		cpuUtil = float64(cpuNs) * 100.0 / float64(wallNs)
	}

	allocCalls := mAfter.Mallocs - mBefore.Mallocs
	allocBytes := mAfter.TotalAlloc - mBefore.TotalAlloc

	res := BenchResult{
		SchemaVersion:       2,
		Implementation:      "maxminddb-golang",
		Language:            "Go",
		Version:             Version,
		Revision:            cfg.Revision,
		Scenario:            "open_file",
		Operation:           "open",
		Threads:             1,
		WorkloadSize:        cfg.OpenIterations,
		WarmupOps:           warmupOps,
		MeasurementOps:      measurementOps,
		WarmupDurationNs:    uint64(time.Second),
		MeasurementWindowNs: wallNs,
		LatencySampleCount:  len(samples),
		MeanNs:              stats.Mean,
		MedianNs:            stats.Median,
		MinNs:               stats.Min,
		MaxNs:               stats.Max,
		P50Ns:               stats.P50,
		P95Ns:               stats.P95,
		P99Ns:               stats.P99,
		VarianceNs2:         stats.Variance,
		StddevNs:            stats.Stddev,
		ThroughputOpsS:      throughput,
		WallTimeNs:          wallNs,
		CpuTimeNs:           cpuNs,
		CpuUtilizationPct:   cpuUtil,
		AllocationCalls:     allocCalls,
		AllocationsPerOp:    float64(allocCalls) / float64(measurementOps),
		AllocatedBytes:      allocBytes,
		AllocatedBytesPerOp: float64(allocBytes) / float64(measurementOps),
		RssBeforeBytes:      rssBefore,
		RssAfterBytes:       rssAfter,
		RssDeltaBytes:       int64(rssAfter) - int64(rssBefore),
		PeakRssBytes:        peakRss,
		FirstOperationNs:    firstOpNs,
		TimerOverheadNs:     timerOverhead,
		DatasetBytes:        datasetBytes,
		Checksum:            checksum,
	}

	enc := json.NewEncoder(os.Stdout)
	return enc.Encode(res)
}

func benchOpen(cfg *Config) error {
	fi, err := os.Stat(cfg.Dataset)
	if err != nil {
		return err
	}
	datasetBytes := uint64(fi.Size())
	timerOverhead := measureTimerOverhead()

	var fileBytes []byte
	if cfg.Scenario == "open_buffer" {
		fileBytes, err = os.ReadFile(cfg.Dataset)
		if err != nil {
			return err
		}
	}

	var firstOpNs uint64
	var checksum uint64
	open := func() (*maxminddb.Reader, error) {
		if cfg.Scenario == "open_buffer" {
			return maxminddb.OpenBytes(fileBytes)
		}
		return maxminddb.Open(cfg.Dataset)
	}
	warmStart := time.Now()
	warmupOps := 0
	for time.Since(warmStart) < time.Second {
		db, openErr := open()
		if openErr != nil {
			return openErr
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		warmupOps++
	}
	samples := make([]uint64, 0, cfg.OpenIterations)

	for i := 0; i < cfg.OpenIterations; i++ {
		t0 := time.Now()
		db, openErr := open()
		dt := uint64(time.Since(t0).Nanoseconds())
		if openErr != nil {
			return openErr
		}
		if i == 0 {
			firstOpNs = dt
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		samples = append(samples, dt)
	}
	rssBefore := currentRssBytes()
	var mBefore, mAfter runtime.MemStats
	runtime.ReadMemStats(&mBefore)
	cpuBefore := processCpuNs()
	wallStart := time.Now()
	measurementOps := 0
	for time.Since(wallStart) < 2*time.Second || measurementOps == 0 {
		db, openErr := open()
		if openErr != nil {
			return openErr
		}
		checksum ^= uint64(db.Metadata.NodeCount)
		db.Close()
		measurementOps++
	}

	wallNs := uint64(time.Since(wallStart).Nanoseconds())
	cpuNs := processCpuNs() - cpuBefore
	runtime.ReadMemStats(&mAfter)
	rssAfter := currentRssBytes()
	peakRss := peakRssBytes()

	stats := summarize(samples)
	throughput := float64(measurementOps) * 1e9 / float64(wallNs)
	cpuUtil := 0.0
	if wallNs > 0 {
		cpuUtil = float64(cpuNs) * 100.0 / float64(wallNs)
	}

	allocCalls := mAfter.Mallocs - mBefore.Mallocs
	allocBytes := mAfter.TotalAlloc - mBefore.TotalAlloc

	res := BenchResult{
		SchemaVersion:       2,
		Implementation:      "maxminddb-golang",
		Language:            "Go",
		Version:             Version,
		Revision:            cfg.Revision,
		Scenario:            cfg.Scenario,
		Operation:           "open",
		Threads:             1,
		WorkloadSize:        cfg.OpenIterations,
		WarmupOps:           warmupOps,
		MeasurementOps:      measurementOps,
		WarmupDurationNs:    uint64(time.Second),
		MeasurementWindowNs: wallNs,
		LatencySampleCount:  len(samples),
		MeanNs:              stats.Mean,
		MedianNs:            stats.Median,
		MinNs:               stats.Min,
		MaxNs:               stats.Max,
		P50Ns:               stats.P50,
		P95Ns:               stats.P95,
		P99Ns:               stats.P99,
		VarianceNs2:         stats.Variance,
		StddevNs:            stats.Stddev,
		ThroughputOpsS:      throughput,
		WallTimeNs:          wallNs,
		CpuTimeNs:           cpuNs,
		CpuUtilizationPct:   cpuUtil,
		AllocationCalls:     allocCalls,
		AllocationsPerOp:    float64(allocCalls) / float64(measurementOps),
		AllocatedBytes:      allocBytes,
		AllocatedBytesPerOp: float64(allocBytes) / float64(measurementOps),
		RssBeforeBytes:      rssBefore,
		RssAfterBytes:       rssAfter,
		RssDeltaBytes:       int64(rssAfter) - int64(rssBefore),
		PeakRssBytes:        peakRss,
		FirstOperationNs:    firstOpNs,
		TimerOverheadNs:     timerOverhead,
		DatasetBytes:        datasetBytes,
		Checksum:            checksum,
	}

	enc := json.NewEncoder(os.Stdout)
	return enc.Encode(res)
}

func benchLookup(cfg *Config) error {
	fi, err := os.Stat(cfg.Dataset)
	if err != nil {
		return err
	}
	datasetBytes := uint64(fi.Size())
	timerOverhead := measureTimerOverhead()

	ips, err := loadWorkload(cfg)
	if err != nil {
		return err
	}

	db, err := maxminddb.Open(cfg.Dataset)
	if err != nil {
		return err
	}
	defer db.Close()

	// Validate the complete absent workload outside the measured sections.
	if cfg.Pattern == "absent" {
		for i, ip := range ips {
			result := db.Lookup(ip)
			if result.Err() != nil || result.Found() {
				return fmt.Errorf("absent preflight failed at query %d: %v", i, result.Err())
			}
		}
	}

	// 1. Measure first operation
	var record CityRecord
	tFirst0 := time.Now()
	_ = db.Lookup(ips[0]).Decode(&record)
	firstOpNs := uint64(time.Since(tFirst0).Nanoseconds())

	// 2. Warmup
	warmStart := time.Now()
	warmupOps := 0
	for time.Since(warmStart) < time.Second {
		i := warmupOps
		_ = db.Lookup(ips[i%len(ips)]).Decode(&record)
		warmupOps++
	}

	// 3. Latency samples
	sampleCount := cfg.LatencySamplesMax
	if sampleCount > cfg.WorkloadSize {
		sampleCount = cfg.WorkloadSize
	}
	sampleStride := 1
	if cfg.WorkloadSize > sampleCount {
		sampleStride = cfg.WorkloadSize / sampleCount
	}
	samples := make([]uint64, 0, sampleCount)
	for i := 0; i < cfg.WorkloadSize; i += sampleStride {
		t0 := time.Now()
		_ = db.Lookup(ips[i]).Decode(&record)
		dt := uint64(time.Since(t0).Nanoseconds())
		samples = append(samples, dt)
		if len(samples) >= sampleCount {
			break
		}
	}

	// 4. Timed bulk loop
	rssBefore := currentRssBytes()
	var mBefore, mAfter runtime.MemStats
	runtime.ReadMemStats(&mBefore)
	cpuBefore := processCpuNs()
	wallStart := time.Now()

	var checksum uint64
	measurementOps := 0
	for time.Since(wallStart) < 2*time.Second || measurementOps == 0 {
		for i := range ips {
			_ = db.Lookup(ips[i]).Decode(&record)
			checksum ^= checksumRecord(&record)
			measurementOps++
		}
	}

	wallNs := uint64(time.Since(wallStart).Nanoseconds())
	cpuNs := processCpuNs() - cpuBefore
	runtime.ReadMemStats(&mAfter)
	rssAfter := currentRssBytes()
	peakRss := peakRssBytes()

	stats := summarize(samples)
	throughput := float64(measurementOps) * 1e9 / float64(wallNs)
	cpuUtil := 0.0
	if wallNs > 0 {
		cpuUtil = float64(cpuNs) * 100.0 / float64(wallNs)
	}

	allocCalls := mAfter.Mallocs - mBefore.Mallocs
	allocBytes := mAfter.TotalAlloc - mBefore.TotalAlloc

	fam := cfg.Family
	pat := cfg.Pattern

	res := BenchResult{
		SchemaVersion:       2,
		Implementation:      "maxminddb-golang",
		Language:            "Go",
		Version:             Version,
		Revision:            cfg.Revision,
		Scenario:            cfg.Scenario,
		Operation:           "lookup_decode",
		Threads:             1,
		Family:              &fam,
		Pattern:             &pat,
		WorkloadSize:        cfg.WorkloadSize,
		WarmupOps:           warmupOps,
		MeasurementOps:      measurementOps,
		WarmupDurationNs:    uint64(time.Second),
		MeasurementWindowNs: wallNs,
		LatencySampleCount:  len(samples),
		MeanNs:              stats.Mean,
		MedianNs:            stats.Median,
		MinNs:               stats.Min,
		MaxNs:               stats.Max,
		P50Ns:               stats.P50,
		P95Ns:               stats.P95,
		P99Ns:               stats.P99,
		VarianceNs2:         stats.Variance,
		StddevNs:            stats.Stddev,
		ThroughputOpsS:      throughput,
		WallTimeNs:          wallNs,
		CpuTimeNs:           cpuNs,
		CpuUtilizationPct:   cpuUtil,
		AllocationCalls:     allocCalls,
		AllocationsPerOp:    float64(allocCalls) / float64(measurementOps),
		AllocatedBytes:      allocBytes,
		AllocatedBytesPerOp: float64(allocBytes) / float64(measurementOps),
		RssBeforeBytes:      rssBefore,
		RssAfterBytes:       rssAfter,
		RssDeltaBytes:       int64(rssAfter) - int64(rssBefore),
		PeakRssBytes:        peakRss,
		FirstOperationNs:    firstOpNs,
		TimerOverheadNs:     timerOverhead,
		DatasetBytes:        datasetBytes,
		Checksum:            checksum,
	}

	enc := json.NewEncoder(os.Stdout)
	return enc.Encode(res)
}

func benchLookupConcurrent(cfg *Config) error {
	fi, err := os.Stat(cfg.Dataset)
	if err != nil {
		return err
	}
	datasetBytes := uint64(fi.Size())
	timerOverhead := measureTimerOverhead()

	ips, err := loadWorkload(cfg)
	if err != nil {
		return err
	}

	db, err := maxminddb.Open(cfg.Dataset)
	if err != nil {
		return err
	}
	defer db.Close()

	// Warm all workers for the same wall-clock interval before collecting samples.
	warmStart := time.Now()
	warmDeadline := warmStart.Add(time.Second + 10*time.Millisecond)
	var warmOps uint64
	var warmWg sync.WaitGroup
	warmWg.Add(cfg.Threads)
	for t := 0; t < cfg.Threads; t++ {
		go func(threadID int) {
			defer warmWg.Done()
			var record CityRecord
			count := uint64(0)
			for time.Now().Before(warmDeadline) {
				_ = db.Lookup(ips[(threadID+int(count))%len(ips)]).Decode(&record)
				count++
			}
			atomic.AddUint64(&warmOps, count)
		}(t)
	}
	warmWg.Wait()

	numThreads := cfg.Threads
	opsPerThread := cfg.WorkloadSize / numThreads
	if opsPerThread == 0 {
		opsPerThread = 1
	}
	totalOps := opsPerThread * numThreads

	samplesPerThread := cfg.LatencySamplesMax / numThreads
	if samplesPerThread == 0 {
		samplesPerThread = 1
	}

	threadSamples := make([][]uint64, numThreads)
	for i := range threadSamples {
		threadSamples[i] = make([]uint64, 0, samplesPerThread)
	}

	// Collect concurrent latency samples after warm-up and before the timed
	// throughput window, so per-operation timers do not bias throughput.
	sampleGo := make(chan struct{})
	var sampleWg sync.WaitGroup
	sampleWg.Add(numThreads)
	for t := 0; t < numThreads; t++ {
		go func(threadID int) {
			defer sampleWg.Done()
			<-sampleGo
			var record CityRecord
			samples := threadSamples[threadID]
			for i := 0; i < samplesPerThread; i++ {
				query := (threadID*samplesPerThread + i) * len(ips) / (samplesPerThread * numThreads)
				start := time.Now()
				_ = db.Lookup(ips[query]).Decode(&record)
				samples = append(samples, uint64(time.Since(start).Nanoseconds()))
			}
			threadSamples[threadID] = samples
		}(t)
	}
	close(sampleGo)
	sampleWg.Wait()

	var globalChecksum uint64
	var measurementOps uint64

	rssBefore := currentRssBytes()
	var mBefore, mAfter runtime.MemStats
	runtime.ReadMemStats(&mBefore)
	cpuBefore := processCpuNs()

	startBarrier := make(chan struct{})
	var wg sync.WaitGroup
	wg.Add(numThreads)

	for t := 0; t < numThreads; t++ {
		go func(threadID int) {
			defer wg.Done()
			var localRec CityRecord
			var localChecksum uint64
			localOps := uint64(0)

			// Wait for start
			<-startBarrier

			deadline := time.Now().Add(time.Second + 10*time.Millisecond)
			for i := 0; time.Now().Before(deadline) || i == 0; i++ {
				idx := (threadID*opsPerThread + i) % len(ips)
				_ = db.Lookup(ips[idx]).Decode(&localRec)
				localChecksum ^= checksumRecord(&localRec)
				localOps++
			}
			atomic.AddUint64(&globalChecksum, localChecksum)
			atomic.AddUint64(&measurementOps, localOps)
		}(t)
	}

	wallStart := time.Now()
	close(startBarrier)
	wg.Wait()
	wallNs := uint64(time.Since(wallStart).Nanoseconds())

	cpuNs := processCpuNs() - cpuBefore
	runtime.ReadMemStats(&mAfter)
	rssAfter := currentRssBytes()
	peakRss := peakRssBytes()

	var allSamples []uint64
	for _, s := range threadSamples {
		allSamples = append(allSamples, s...)
	}
	stats := summarize(allSamples)
	throughput := float64(measurementOps) * 1e9 / float64(wallNs)
	cpuUtil := 0.0
	if wallNs > 0 {
		cpuUtil = float64(cpuNs) * 100.0 / float64(wallNs)
	}

	allocCalls := mAfter.Mallocs - mBefore.Mallocs
	allocBytes := mAfter.TotalAlloc - mBefore.TotalAlloc

	fam := cfg.Family
	pat := cfg.Pattern

	res := BenchResult{
		SchemaVersion:       2,
		Implementation:      "maxminddb-golang",
		Language:            "Go",
		Version:             Version,
		Revision:            cfg.Revision,
		Scenario:            cfg.Scenario,
		Operation:           "lookup_decode",
		Threads:             numThreads,
		Family:              &fam,
		Pattern:             &pat,
		WorkloadSize:        totalOps,
		WarmupOps:           int(warmOps),
		MeasurementOps:      int(measurementOps),
		WarmupDurationNs:    uint64(time.Second + 10*time.Millisecond),
		MeasurementWindowNs: wallNs,
		LatencySampleCount:  len(allSamples),
		MeanNs:              stats.Mean,
		MedianNs:            stats.Median,
		MinNs:               stats.Min,
		MaxNs:               stats.Max,
		P50Ns:               stats.P50,
		P95Ns:               stats.P95,
		P99Ns:               stats.P99,
		VarianceNs2:         stats.Variance,
		StddevNs:            stats.Stddev,
		ThroughputOpsS:      throughput,
		WallTimeNs:          wallNs,
		CpuTimeNs:           cpuNs,
		CpuUtilizationPct:   cpuUtil,
		AllocationCalls:     allocCalls,
		AllocationsPerOp:    float64(allocCalls) / float64(measurementOps),
		AllocatedBytes:      allocBytes,
		AllocatedBytesPerOp: float64(allocBytes) / float64(measurementOps),
		RssBeforeBytes:      rssBefore,
		RssAfterBytes:       rssAfter,
		RssDeltaBytes:       int64(rssAfter) - int64(rssBefore),
		PeakRssBytes:        peakRss,
		FirstOperationNs:    0,
		TimerOverheadNs:     timerOverhead,
		DatasetBytes:        datasetBytes,
		Checksum:            globalChecksum,
	}

	enc := json.NewEncoder(os.Stdout)
	return enc.Encode(res)
}

func main() {
	cfg, err := loadConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "config error: %v\n", err)
		os.Exit(1)
	}

	switch cfg.Scenario {
	case "open_file":
		// For maxminddb-golang, open_file reads the file into memory and uses OpenBytes
		// This is equivalent to open_buffer but reads from file path
		if err := benchOpenFromFile(cfg); err != nil {
			fmt.Fprintf(os.Stderr, "open_file bench error: %v\n", err)
			os.Exit(1)
		}
	case "open_buffer", "open_mmap":
		if err := benchOpen(cfg); err != nil {
			fmt.Fprintf(os.Stderr, "open bench error: %v\n", err)
			os.Exit(1)
		}
	case "lookup_concurrent":
		if err := benchLookupConcurrent(cfg); err != nil {
			fmt.Fprintf(os.Stderr, "concurrent lookup bench error: %v\n", err)
			os.Exit(1)
		}
	case "lookup":
		if err := benchLookup(cfg); err != nil {
			fmt.Fprintf(os.Stderr, "lookup bench error: %v\n", err)
			os.Exit(1)
		}
	default:
		fmt.Fprintf(os.Stderr, "unknown scenario: %s\n", cfg.Scenario)
		os.Exit(1)
	}
}
