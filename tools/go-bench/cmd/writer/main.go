package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"net"
	"os"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/maxmind/mmdbwriter"
	"github.com/maxmind/mmdbwriter/mmdbtype"
)

type SplitMix64 struct {
	state uint64
}

func (s *SplitMix64) NextU64() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *SplitMix64) NextU32() uint32 {
	return uint32(s.NextU64() >> 32)
}

func (s *SplitMix64) NextIPv4Net() *net.IPNet {
	ipU32 := s.NextU32()
	prefix := 16 + int(s.NextU32()%16) // /16 to /31
	mask := uint32(0)
	if prefix > 0 {
		mask = ^uint32(0) << (32 - prefix)
	}
	netIP := ipU32 & mask
	ip := net.IPv4(
		byte(netIP>>24),
		byte(netIP>>16),
		byte(netIP>>8),
		byte(netIP),
	).To4()
	return &net.IPNet{
		IP:   ip,
		Mask: net.CIDRMask(prefix, 32),
	}
}

func (s *SplitMix64) NextIPv6Net() *net.IPNet {
	hi := s.NextU64()
	lo := s.NextU64()
	prefix := 48 + int(s.NextU32()%48) // /48 to /95

	ipBytes := make([]byte, 16)
	for i := 0; i < 8; i++ {
		ipBytes[i] = byte(hi >> (56 - i*8))
		ipBytes[8+i] = byte(lo >> (56 - i*8))
	}
	mask := net.CIDRMask(prefix, 128)
	for i := 0; i < 16; i++ {
		ipBytes[i] &= mask[i]
	}
	return &net.IPNet{
		IP:   net.IP(ipBytes),
		Mask: mask,
	}
}

func parseSeed(s string) uint64 {
	s = strings.TrimSpace(s)
	if strings.HasPrefix(strings.ToLower(s), "0x") {
		v, err := strconv.ParseUint(s[2:], 16, 64)
		if err == nil {
			return v
		}
	}
	v, err := strconv.ParseUint(s, 10, 64)
	if err == nil {
		return v
	}
	return 0x5EED2024CAFEBABE
}

func sharedRecord() mmdbtype.DataType {
	return mmdbtype.Map{
		mmdbtype.String("city"): mmdbtype.Map{
			mmdbtype.String("geoname_id"): mmdbtype.Uint32(2643743),
			mmdbtype.String("names"): mmdbtype.Map{
				mmdbtype.String("en"): mmdbtype.String("Benchmark City"),
			},
		},
		mmdbtype.String("continent"): mmdbtype.Map{
			mmdbtype.String("code"):       mmdbtype.String("NA"),
			mmdbtype.String("geoname_id"): mmdbtype.Uint32(6255149),
			mmdbtype.String("names"): mmdbtype.Map{
				mmdbtype.String("en"): mmdbtype.String("North America"),
			},
		},
		mmdbtype.String("country"): mmdbtype.Map{
			mmdbtype.String("geoname_id"): mmdbtype.Uint32(6252001),
			mmdbtype.String("iso_code"):   mmdbtype.String("US"),
			mmdbtype.String("names"): mmdbtype.Map{
				mmdbtype.String("en"): mmdbtype.String("United States"),
			},
		},
		mmdbtype.String("location"): mmdbtype.Map{
			mmdbtype.String("accuracy_radius"): mmdbtype.Uint16(50),
			mmdbtype.String("latitude"):        mmdbtype.Float64(37.751),
			mmdbtype.String("longitude"):       mmdbtype.Float64(-122.42),
			mmdbtype.String("time_zone"):       mmdbtype.String("America/Los_Angeles"),
		},
	}
}

func currentRSSBytes() uint64 {
	data, err := os.ReadFile("/proc/self/statm")
	if err == nil {
		fields := strings.Fields(string(data))
		if len(fields) >= 2 {
			if pages, err := strconv.ParseUint(fields[1], 10, 64); err == nil {
				return pages * uint64(os.Getpagesize())
			}
		}
	}
	return peakRSSBytes()
}

func peakRSSBytes() uint64 {
	var ru syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &ru); err == nil {
		return uint64(ru.Maxrss) * 1024
	}
	return 0
}

func percentile(sorted []uint64, q float64) uint64 {
	if len(sorted) == 0 {
		return 0
	}
	idx := int(float64(len(sorted)-1)*q + 0.5)
	if idx >= len(sorted) {
		idx = len(sorted) - 1
	}
	return sorted[idx]
}

func main() {
	var size int
	var entries int
	var batchSize int
	var family string
	var seedStr string
	var outputPath string

	flag.IntVar(&size, "size", 0, "number of IP entries to insert")
	flag.IntVar(&entries, "entries", 0, "number of IP entries to insert")
	flag.IntVar(&batchSize, "batch-size", 25000, "batch size for insertion")
	flag.IntVar(&batchSize, "batch", 25000, "batch size for insertion")
	flag.StringVar(&family, "family", "ipv4", "IP family: ipv4 or ipv6")
	flag.StringVar(&seedStr, "seed", "0x5EED2024CAFEBABE", "hex seed for deterministic PRNG")
	flag.StringVar(&outputPath, "output", "", "optional output path for the MMDB file")
	flag.Parse()

	if size == 0 {
		if entries > 0 {
			size = entries
		} else {
			size = 10000
		}
	}

	if batchSize <= 0 {
		batchSize = 25000
	}

	seed := parseSeed(seedStr)
	family = strings.ToLower(family)
	ipVersion := 4
	if family == "ipv6" || family == "6" {
		ipVersion = 6
	}

	tree, err := mmdbwriter.New(mmdbwriter.Options{
		DatabaseType:            "GeoIP2-City",
		IPVersion:               ipVersion,
		RecordSize:              28,
		IncludeReservedNetworks: true,
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to create tree: %v\n", err)
		os.Exit(1)
	}

	rec := sharedRecord()
	rng := SplitMix64{state: seed}

	maxSamples := 25000
	if size < maxSamples {
		maxSamples = size
	}
	sampleStride := 1
	if size > maxSamples {
		sampleStride = size / maxSamples
	}
	sampleLatencies := make([]uint64, 0, maxSamples+16)

	rssBefore := currentRSSBytes()
	var rssSum uint64
	var rssSamples uint64
	tTotalStart := time.Now()

	// Insertion loop in batches
	batch := make([]*net.IPNet, batchSize)
	tInsertStart := time.Now()
	for i := 0; i < size; i += batchSize {
		chunkSize := batchSize
		if i+chunkSize > size {
			chunkSize = size - i
		}

		for j := 0; j < chunkSize; j++ {
			if ipVersion == 4 {
				batch[j] = rng.NextIPv4Net()
			} else {
				batch[j] = rng.NextIPv6Net()
			}
		}

		for j := 0; j < chunkSize; j++ {
			idx := i + j
			netw := batch[j]
			if idx%sampleStride == 0 && len(sampleLatencies) < maxSamples {
				t0 := time.Now()
				if err := tree.Insert(netw, rec); err != nil {
					fmt.Fprintf(os.Stderr, "insert error at #%d: %v\n", idx, err)
					os.Exit(1)
				}
				elapsed := uint64(time.Since(t0).Nanoseconds())
				sampleLatencies = append(sampleLatencies, elapsed)
			} else {
				if err := tree.Insert(netw, rec); err != nil {
					fmt.Fprintf(os.Stderr, "insert error at #%d: %v\n", idx, err)
					os.Exit(1)
				}
			}
		}

		curRSS := currentRSSBytes()
		rssSum += curRSS
		rssSamples++
	}
	insertTimeNs := uint64(time.Since(tInsertStart).Nanoseconds())

	// Build / Write MMDB
	tBuildStart := time.Now()
	var writtenBytes int64
	targetPath := outputPath
	isTemp := false
	if targetPath == "" {
		isTemp = true
		tmpFile, err := os.CreateTemp("", "mmdbwriter-bench-*.mmdb")
		if err != nil {
			fmt.Fprintf(os.Stderr, "failed to create temp file: %v\n", err)
			os.Exit(1)
		}
		targetPath = tmpFile.Name()
		tmpFile.Close()
	}

	f, err := os.Create(targetPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to create output file: %v\n", err)
		os.Exit(1)
	}
	written, err := tree.WriteTo(f)
	if err != nil {
		f.Close()
		fmt.Fprintf(os.Stderr, "failed to write MMDB: %v\n", err)
		os.Exit(1)
	}
	_ = f.Sync()
	f.Close()
	writtenBytes = written
	if isTemp {
		_ = os.Remove(targetPath)
	}
	buildTimeNs := uint64(time.Since(tBuildStart).Nanoseconds())

	wallTimeNs := uint64(time.Since(tTotalStart).Nanoseconds())
	rssAfter := currentRSSBytes()
	peakRSS := peakRSSBytes()
	rssDelta := int64(rssAfter) - int64(rssBefore)

	// Latency statistics
	sort.Slice(sampleLatencies, func(i, j int) bool { return sampleLatencies[i] < sampleLatencies[j] })
	var sum uint64
	for _, v := range sampleLatencies {
		sum += v
	}
	n := float64(len(sampleLatencies))
	meanNs := 0.0
	if n > 0 {
		meanNs = float64(sum) / n
	}
	variance := 0.0
	if n > 1 {
		var sumSquares float64
		for _, v := range sampleLatencies {
			diff := float64(v) - meanNs
			sumSquares += diff * diff
		}
		variance = sumSquares / (n - 1.0)
	}

	p50Ns := percentile(sampleLatencies, 0.50)
	p95Ns := percentile(sampleLatencies, 0.95)
	p99Ns := percentile(sampleLatencies, 0.99)
	minNs := uint64(0)
	maxNs := uint64(0)
	if len(sampleLatencies) > 0 {
		minNs = sampleLatencies[0]
		maxNs = sampleLatencies[len(sampleLatencies)-1]
	}

	tpOpsS := 0.0
	if wallTimeNs > 0 {
		tpOpsS = (float64(size) / float64(wallTimeNs)) * 1e9
	}

	avgRSSBytes := uint64(0)
	if rssSamples > 0 {
		avgRSSBytes = rssSum / rssSamples
	} else {
		avgRSSBytes = rssAfter
	}

	result := map[string]interface{}{
		"schema_version":   2,
		"implementation":   "mmdbwriter",
		"language":         "go",
		"version":          "v1.2.0",
		"scenario":         "writer_generation",
		"family":           family,
		"database_size":    size,
		"workload_size":    size,
		"entries":          size,
		"batch_size":       batchSize,
		"wall_time_ns":     wallTimeNs,
		"insert_time_ns":   insertTimeNs,
		"build_time_ns":    buildTimeNs,
		"throughput_ops_s": tpOpsS,
		"mean_ns":          meanNs,
		"median_ns":        float64(p50Ns),
		"min_ns":           minNs,
		"max_ns":           maxNs,
		"p50_ns":           p50Ns,
		"p95_ns":           p95Ns,
		"p99_ns":           p99Ns,
		"stddev_ns":        math.Sqrt(variance),
		"database_bytes":   uint64(writtenBytes),
		"rss_before_bytes": rssBefore,
		"rss_after_bytes":  rssAfter,
		"rss_delta_bytes":  rssDelta,
		"avg_rss_bytes":    avgRSSBytes,
		"peak_rss_bytes":   peakRSS,
	}

	out, _ := json.Marshal(result)
	fmt.Println(string(out))
}
