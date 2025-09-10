// Standard library imports
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

// External crate imports
use chrono::{DateTime, Utc};
use clap::Parser;
use hdrhistogram::Histogram;
use humantime::parse_duration;
use indicatif::{ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use parking_lot::Mutex;
use prometheus::{
    Gauge, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use tokio::time::{interval, sleep};

// Remote write module
mod remote_write;
use remote_write::RemoteWriteClient;

// =============================================================================
// PROMETHEUS METRICS
// =============================================================================

lazy_static! {
    static ref REGISTRY: Registry = Registry::new();
    static ref REMOTE_WRITE_CLIENT: parking_lot::Mutex<Option<RemoteWriteClient>> = parking_lot::Mutex::new(None);

    // Request metrics
    static ref REQUEST_COUNTER: IntCounterVec = IntCounterVec::new(
        Opts::new("forgy_requests_total", "Total number of requests made"),
        &["status", "method"]
    ).unwrap();

    static ref REQUEST_DURATION: HistogramVec = HistogramVec::new(
        HistogramOpts::new("forgy_request_duration_seconds", "Request duration in seconds")
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        &["method", "status_class"]
    ).unwrap();

    static ref ACTIVE_VUS: IntGauge = IntGauge::new(
        "forgy_active_vus", "Number of active virtual users"
    ).unwrap();

    static ref TARGET_VUS: IntGauge = IntGauge::new(
        "forgy_target_vus", "Target number of virtual users"
    ).unwrap();

    static ref SUCCESS_RATE: Gauge = Gauge::new(
        "forgy_success_rate", "Current success rate (percentage)"
    ).unwrap();

    static ref REQUESTS_PER_SECOND: Gauge = Gauge::new(
        "forgy_requests_per_second", "Current requests per second"
    ).unwrap();

    // Response time percentiles
    static ref RESPONSE_TIME_P50: Gauge = Gauge::new(
        "forgy_response_time_p50_ms", "50th percentile response time in milliseconds"
    ).unwrap();

    static ref RESPONSE_TIME_P90: Gauge = Gauge::new(
        "forgy_response_time_p90_ms", "90th percentile response time in milliseconds"
    ).unwrap();

    static ref RESPONSE_TIME_P95: Gauge = Gauge::new(
        "forgy_response_time_p95_ms", "95th percentile response time in milliseconds"
    ).unwrap();

    static ref RESPONSE_TIME_P99: Gauge = Gauge::new(
        "forgy_response_time_p99_ms", "99th percentile response time in milliseconds"
    ).unwrap();

    // Test phase indicator
    static ref TEST_PHASE: IntGaugeVec = IntGaugeVec::new(
        Opts::new("forgy_phase", "Current test phase (0=idle, 1=rampup, 2=hold, 3=rampdown)"),
        &["phase"]
    ).unwrap();

    // Data transfer metrics
    static ref DATA_SENT: IntCounterVec = IntCounterVec::new(
        Opts::new("forgy_data_sent", "Total number of bytes sent in HTTP requests"),
        &["method"]
    ).unwrap();

    static ref DATA_RECEIVED: IntCounterVec = IntCounterVec::new(
        Opts::new("forgy_data_received", "Total number of bytes received in HTTP responses"),
        &["method", "status_class"]
    ).unwrap();
}

// =============================================================================
// DATA STRUCTURES
// =============================================================================

#[derive(Parser, Debug)]
#[clap(name = "forgy")]
#[clap(about = "High-performance REST endpoint load testing tool with Prometheus metrics", long_about = None)]
struct Args {
    /// Target URL to test
    #[clap(long, value_parser)]
    url: String,

    /// Number of virtual users (concurrent connections)
    #[clap(long, default_value = "10")]
    vus: usize,

    /// Ramp-up duration (e.g., 5m, 30s, 1h)
    #[clap(long, default_value = "10s")]
    ramp_up: String,

    /// Hold duration at peak load (e.g., 1h, 30m, 60s)
    #[clap(long, default_value = "30s")]
    hold: String,

    /// Ramp-down duration (e.g., 60s, 5m)
    #[clap(long, default_value = "10s")]
    ramp_down: String,

    /// HTTP method to use
    #[clap(long, default_value = "GET")]
    method: String,

    /// Request body (for POST/PUT requests)
    #[clap(long)]
    body: Option<String>,

    /// Headers in format "Key:Value" (can be used multiple times)
    #[clap(long)]
    header: Vec<String>,

    /// Request timeout in seconds
    #[clap(long, default_value = "30")]
    timeout: u64,

    /// Output results to JSON file
    #[clap(long)]
    output: Option<String>,

    /// Prometheus Remote Write URL (e.g., http://localhost:9090/api/v1/write)
    #[clap(long, value_name = "URL")]
    prometheus_url: Option<String>,

    /// Application label for grouping metrics in Prometheus (default: forgy)
    #[clap(long, default_value = "forgy")]
    app: String,

    /// Metrics push frequency in seconds (default: 10)
    #[clap(long, default_value = "10")]
    metrics_frequency: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestStats {
    success: bool,
    status_code: u16,
    duration_ms: f64,
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct TestResults {
    total_requests: usize,
    successful_requests: usize,
    failed_requests: usize,
    vus: usize,
    avg_response_time_ms: f64,
    min_response_time_ms: f64,
    max_response_time_ms: f64,
    p50_response_time_ms: f64,
    p90_response_time_ms: f64,
    p95_response_time_ms: f64,
    p99_response_time_ms: f64,
    requests_per_second: f64,
    test_duration_seconds: f64,
    status_code_distribution: HashMap<u16, usize>,
    total_bytes_sent: u64,
    total_bytes_received: u64,
}

// =============================================================================
// LOAD TESTER
// =============================================================================

struct LoadTester {
    client: Client,
    url: String,
    method: Method,
    body: Option<String>,
    stats: Arc<Mutex<Vec<RequestStats>>>,
    active_vus: Arc<Mutex<usize>>,
    histogram: Arc<Mutex<Histogram<u64>>>,
    status_codes: Arc<Mutex<HashMap<u16, usize>>>,
    total_requests: Arc<Mutex<usize>>,
    successful_requests: Arc<Mutex<usize>>,
    total_bytes_sent: Arc<Mutex<u64>>,
    total_bytes_received: Arc<Mutex<u64>>,
}

impl LoadTester {
    fn new(args: &Args) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        for header in &args.header {
            if let Some((key, value)) = header.split_once(':') {
                if let (Ok(name), Ok(val)) = (
                    reqwest::header::HeaderName::from_bytes(key.trim().as_bytes()),
                    reqwest::header::HeaderValue::from_str(value.trim()),
                ) {
                    headers.insert(name, val);
                }
            }
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(args.timeout))
            .pool_max_idle_per_host(args.vus)
            .build()
            .expect("Failed to create HTTP client");

        let method = Method::from_bytes(args.method.as_bytes()).unwrap_or(Method::GET);

        Self {
            client,
            url: args.url.clone(),
            method,
            body: args.body.clone(),
            stats: Arc::new(Mutex::new(Vec::new())),
            active_vus: Arc::new(Mutex::new(0)),
            histogram: Arc::new(Mutex::new(Histogram::<u64>::new(3).unwrap())),
            status_codes: Arc::new(Mutex::new(HashMap::new())),
            total_requests: Arc::new(Mutex::new(0)),
            successful_requests: Arc::new(Mutex::new(0)),
            total_bytes_sent: Arc::new(Mutex::new(0)),
            total_bytes_received: Arc::new(Mutex::new(0)),
        }
    }

    async fn make_request(&self, prometheus_enabled: bool) -> RequestStats {
        let start = Instant::now();
        let timestamp = Utc::now();

        let mut request = self.client.request(self.method.clone(), &self.url);

        // Calculate bytes sent
        let mut bytes_sent = 0u64;

        // Calculate request body size
        if let Some(body) = &self.body {
            bytes_sent += body.len() as u64;
            request = request.body(body.clone());
        }

        // Estimate header size (HTTP method + URL + common headers)
        bytes_sent += self.method.as_str().len() as u64; // HTTP method
        bytes_sent += self.url.len() as u64; // URL
        bytes_sent += 150; // Estimate for HTTP headers (Host, User-Agent, Accept, etc.)

        let result = request.send().await;
        let duration = start.elapsed();
        let duration_ms = duration.as_secs_f64() * 1000.0;
        let duration_secs = duration.as_secs_f64();

        let (success, status_code, bytes_received) = match result {
            Ok(response) => {
                let code = response.status().as_u16();
                let is_success = response.status().is_success();
                let mut received_bytes = 0u64;

                // Get response body size
                if let Ok(body) = response.text().await {
                    received_bytes += body.len() as u64;
                }

                // Estimate response headers size
                received_bytes += 200; // Estimate for response headers (Status line, Content-Type, etc.)

                (is_success, code, received_bytes)
            }
            Err(_) => (false, 0, 0),
        };

        // Update Prometheus metrics only if enabled
        if prometheus_enabled {
            let status_str = status_code.to_string();
            let method_str = self.method.as_str();
            REQUEST_COUNTER
                .with_label_values(&[&status_str, method_str])
                .inc();

            let status_class = match status_code {
                200..=299 => "2xx",
                300..=399 => "3xx",
                400..=499 => "4xx",
                500..=599 => "5xx",
                _ => "other",
            };
            REQUEST_DURATION
                .with_label_values(&[method_str, status_class])
                .observe(duration_secs);

            // Update data transfer metrics
            DATA_SENT
                .with_label_values(&[method_str])
                .inc_by(bytes_sent);

            DATA_RECEIVED
                .with_label_values(&[method_str, status_class])
                .inc_by(bytes_received);
        }

        // Update local metrics (record duration in microseconds for better precision)
        let duration_micros = (duration_ms * 1000.0) as u64;
        self.histogram.lock().record(duration_micros).ok();
        *self.status_codes.lock().entry(status_code).or_insert(0) += 1;
        *self.total_requests.lock() += 1;
        if success {
            *self.successful_requests.lock() += 1;
        }

        // Update local byte counters
        *self.total_bytes_sent.lock() += bytes_sent;
        *self.total_bytes_received.lock() += bytes_received;

        RequestStats {
            success,
            status_code,
            duration_ms,
            timestamp,
        }
    }

    async fn run_virtual_user(
        &self,
        stop_signal: Arc<Mutex<bool>>,
        prometheus_enabled: bool,
        vu_index: usize,
    ) {
        *self.active_vus.lock() += 1;
        if prometheus_enabled {
            ACTIVE_VUS.inc();
        }

        // Create a deterministic but distributed offset for this VU
        // Spread VUs evenly across the first second
        let offset_ms = (vu_index * 1000 / 100.max(vu_index + 1)) as u64;

        // Initial delay to spread VUs across the first second
        sleep(Duration::from_millis(offset_ms % 1000)).await;

        while !*stop_signal.lock() {
            let stat = self.make_request(prometheus_enabled).await;
            // Only store detailed stats if needed - limit memory usage for long tests
            {
                let mut stats = self.stats.lock();
                if stats.len() < 50000 {
                    // Cap at 50k samples to prevent excessive memory usage
                    stats.push(stat);
                }
            } // stats lock is released here before the await

            // Wait ~1 second with some jitter to distribute requests
            let base_delay = 1000; // 1 second base
            let jitter = (vu_index * 37) % 400; // Deterministic jitter 0-400ms
            let total_delay = base_delay - 200 + jitter as u64; // 800-1200ms range

            sleep(Duration::from_millis(total_delay)).await;
        }

        *self.active_vus.lock() -= 1;
        if prometheus_enabled {
            ACTIVE_VUS.dec();
        }
    }

    async fn update_and_push_metrics_periodically(
        &self,
        prometheus_url: Option<&str>,
        app: &str,
        frequency_secs: u64,
    ) {
        // Use configurable metrics push frequency
        let mut interval = interval(Duration::from_secs(frequency_secs));
        let mut last_request_count = 0;

        loop {
            interval.tick().await;

            let total = *self.total_requests.lock();
            let successful = *self.successful_requests.lock();

            // Calculate success rate
            if total > 0 {
                let success_rate = (successful as f64 / total as f64) * 100.0;
                SUCCESS_RATE.set(success_rate);
            }

            // Calculate requests per second (since last push)
            let requests_since_last = total - last_request_count;
            REQUESTS_PER_SECOND.set(requests_since_last as f64 / frequency_secs as f64);
            last_request_count = total;

            // Update percentiles
            {
                let histogram = self.histogram.lock();
                if !histogram.is_empty() {
                    // Convert from microseconds to milliseconds for Prometheus metrics
                    RESPONSE_TIME_P50.set(histogram.value_at_percentile(50.0) as f64 / 1000.0);
                    RESPONSE_TIME_P90.set(histogram.value_at_percentile(90.0) as f64 / 1000.0);
                    RESPONSE_TIME_P95.set(histogram.value_at_percentile(95.0) as f64 / 1000.0);
                    RESPONSE_TIME_P99.set(histogram.value_at_percentile(99.0) as f64 / 1000.0);
                }
            }

            // Push metrics via Remote Write if URL is provided
            if let Some(url) = prometheus_url {
                if let Err(e) = send_metrics_via_remote_write(url, app).await {
                    eprintln!("Failed to send metrics via Remote Write: {}", e);
                }
            }
        }
    }

    async fn run_load_test(&self, args: &Args) -> TestResults {
        let ramp_up = parse_duration(&args.ramp_up).expect("Invalid ramp-up duration");
        let hold = parse_duration(&args.hold).expect("Invalid hold duration");
        let ramp_down = parse_duration(&args.ramp_down).expect("Invalid ramp-down duration");

        let total_duration = ramp_up + hold + ramp_down;
        let test_start = Instant::now();
        let prometheus_enabled = args.prometheus_url.is_some();

        println!("\nStarting load test");
        println!("   URL: {}", self.url);
        println!("   Method: {}", self.method);
        println!("   Target VUs: {}", args.vus);
        println!("   Ramp-up: {:?}", ramp_up);
        println!("   Hold: {:?}", hold);
        println!("   Ramp-down: {:?}", ramp_down);
        if prometheus_enabled {
            println!(
                "   Prometheus Remote Write: {}",
                args.prometheus_url.as_ref().unwrap()
            );
            println!("   App Label: {}", args.app);
        }
        println!();

        if prometheus_enabled {
            TARGET_VUS.set(args.vus as i64);
        }

        // Start metrics updater and pusher if Prometheus is enabled
        let metrics_handle = if prometheus_enabled {
            let tester_clone = self.clone();
            let frequency = args.metrics_frequency;
            let prometheus_url = args.prometheus_url.clone();
            let app = args.app.clone();
            Some(tokio::spawn(async move {
                tester_clone
                    .update_and_push_metrics_periodically(
                        prometheus_url.as_deref(),
                        &app,
                        frequency,
                    )
                    .await;
            }))
        } else {
            None
        };

        let pb = ProgressBar::new(total_duration.as_secs());
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40}] [{eta_precise}] {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );

        let mut handles = Vec::new();
        let mut vu_stop_signals: Vec<Arc<Mutex<bool>>> = Vec::new();

        // Ramp-up phase
        if prometheus_enabled {
            TEST_PHASE.with_label_values(&["rampup"]).set(1);
            TEST_PHASE.with_label_values(&["idle"]).set(0);
        }

        let total_ramp_millis = ramp_up.as_millis() as f64;
        let vu_interval_millis = total_ramp_millis / args.vus as f64;
        let mut current_vus = 0;
        let mut next_vu_time = 0.0;
        let mut progress_interval = interval(Duration::from_millis(500)); // Update progress twice per second

        while test_start.elapsed() < ramp_up && current_vus < args.vus {
            let elapsed_millis = test_start.elapsed().as_millis() as f64;

            // Add VUs gradually based on time intervals
            while elapsed_millis >= next_vu_time && current_vus < args.vus {
                let tester = self.clone();
                let vu_stop_signal = Arc::new(Mutex::new(false));
                let stop = vu_stop_signal.clone();
                let vu_index = current_vus;

                vu_stop_signals.push(vu_stop_signal);

                handles.push(tokio::spawn(async move {
                    tester
                        .run_virtual_user(stop, prometheus_enabled, vu_index)
                        .await;
                }));

                current_vus += 1;
                next_vu_time = current_vus as f64 * vu_interval_millis;
            }

            // Update progress less frequently
            tokio::select! {
                _ = progress_interval.tick() => {
                    pb.set_position(test_start.elapsed().as_secs());
                    pb.set_message(format!("{}/{} VUs (ramp-up)", current_vus, args.vus));
                }
                _ = sleep(Duration::from_millis(50)) => {} // Small sleep to prevent busy waiting
            }
        }

        // Hold phase
        if prometheus_enabled {
            TEST_PHASE.with_label_values(&["rampup"]).set(0);
            TEST_PHASE.with_label_values(&["hold"]).set(1);
        }

        let hold_end = test_start.elapsed() + hold;
        while test_start.elapsed() < hold_end {
            sleep(Duration::from_secs(1)).await;
            pb.set_position(test_start.elapsed().as_secs());
            pb.set_message(format!("{}/{} VUs (hold)", args.vus, args.vus));
        }

        // Ramp-down phase
        if prometheus_enabled {
            TEST_PHASE.with_label_values(&["hold"]).set(0);
            TEST_PHASE.with_label_values(&["rampdown"]).set(1);
        }

        let ramp_down_start = test_start.elapsed();
        let total_ramp_down_millis = ramp_down.as_millis() as f64;
        let vu_stop_interval_millis = total_ramp_down_millis / args.vus as f64;
        let mut vus_to_stop = args.vus;
        let mut next_stop_time = 0.0;
        let mut progress_interval = interval(Duration::from_millis(500));

        while test_start.elapsed() < (ramp_down_start + ramp_down) && vus_to_stop > 0 {
            let ramp_down_elapsed_millis =
                (test_start.elapsed() - ramp_down_start).as_millis() as f64;

            // Stop VUs gradually based on time intervals
            while ramp_down_elapsed_millis >= next_stop_time && vus_to_stop > 0 {
                // Stop the oldest VU by setting its individual stop signal
                let vu_to_stop_index = args.vus - vus_to_stop;
                if vu_to_stop_index < vu_stop_signals.len() {
                    *vu_stop_signals[vu_to_stop_index].lock() = true;
                }

                vus_to_stop -= 1;
                next_stop_time = (args.vus - vus_to_stop) as f64 * vu_stop_interval_millis;
            }

            // Update progress less frequently
            tokio::select! {
                _ = progress_interval.tick() => {
                    pb.set_position(test_start.elapsed().as_secs());
                    let remaining_vus = *self.active_vus.lock();
                    pb.set_message(format!("{}/{} VUs (ramp-down)", remaining_vus, args.vus));
                }
                _ = sleep(Duration::from_millis(50)) => {} // Small sleep to prevent busy waiting
            }
        }

        // Ensure all VUs are stopped
        for vu_stop_signal in &vu_stop_signals {
            *vu_stop_signal.lock() = true;
        }
        pb.finish_with_message("Test completed");

        if prometheus_enabled {
            TEST_PHASE.with_label_values(&["rampdown"]).set(0);
            TEST_PHASE.with_label_values(&["idle"]).set(1);
        }

        // Wait for all VUs to finish
        for handle in handles {
            handle.await.ok();
        }

        // Stop metrics updater if it was started
        if let Some(handle) = metrics_handle {
            handle.abort();
        }

        // Calculate results
        self.calculate_results(test_start.elapsed().as_secs_f64(), args.vus)
    }

    fn calculate_results(&self, duration_seconds: f64, vus: usize) -> TestResults {
        let stats = self.stats.lock();
        let histogram = self.histogram.lock();
        let status_codes = self.status_codes.lock().clone();

        let total_requests = stats.len();
        let successful_requests = stats.iter().filter(|s| s.success).count();
        let failed_requests = total_requests - successful_requests;

        let avg_response_time_ms = if total_requests > 0 {
            stats.iter().map(|s| s.duration_ms).sum::<f64>() / total_requests as f64
        } else {
            0.0
        };

        let min_response_time_ms = stats
            .iter()
            .map(|s| s.duration_ms)
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        let max_response_time_ms = stats
            .iter()
            .map(|s| s.duration_ms)
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0);

        // Convert from microseconds back to milliseconds for percentiles
        let p50_response_time_ms = if !histogram.is_empty() {
            histogram.value_at_percentile(50.0) as f64 / 1000.0
        } else {
            0.0
        };
        let p90_response_time_ms = if !histogram.is_empty() {
            histogram.value_at_percentile(90.0) as f64 / 1000.0
        } else {
            0.0
        };
        let p95_response_time_ms = if !histogram.is_empty() {
            histogram.value_at_percentile(95.0) as f64 / 1000.0
        } else {
            0.0
        };
        let p99_response_time_ms = if !histogram.is_empty() {
            histogram.value_at_percentile(99.0) as f64 / 1000.0
        } else {
            0.0
        };

        let requests_per_second = if duration_seconds > 0.0 {
            total_requests as f64 / duration_seconds
        } else {
            0.0
        };

        let total_bytes_sent = *self.total_bytes_sent.lock();
        let total_bytes_received = *self.total_bytes_received.lock();

        TestResults {
            total_requests,
            successful_requests,
            failed_requests,
            vus,
            avg_response_time_ms,
            min_response_time_ms,
            max_response_time_ms,
            p50_response_time_ms,
            p90_response_time_ms,
            p95_response_time_ms,
            p99_response_time_ms,
            requests_per_second,
            test_duration_seconds: duration_seconds,
            status_code_distribution: status_codes,
            total_bytes_sent,
            total_bytes_received,
        }
    }
}

impl Clone for LoadTester {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            url: self.url.clone(),
            method: self.method.clone(),
            body: self.body.clone(),
            stats: self.stats.clone(),
            active_vus: self.active_vus.clone(),
            histogram: self.histogram.clone(),
            status_codes: self.status_codes.clone(),
            total_requests: self.total_requests.clone(),
            successful_requests: self.successful_requests.clone(),
            total_bytes_sent: self.total_bytes_sent.clone(),
            total_bytes_received: self.total_bytes_received.clone(),
        }
    }
}

// =============================================================================
// PROMETHEUS REMOTE WRITE FUNCTIONALITY
// =============================================================================

async fn send_metrics_via_remote_write(
    remote_write_url: &str,
    app: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Get or create the singleton client
    let client = {
        let mut client_guard = REMOTE_WRITE_CLIENT.lock();
        if client_guard.is_none() {
            *client_guard = Some(RemoteWriteClient::new(remote_write_url.to_string()));
        }
        client_guard.as_ref().unwrap().clone()
    };
    client.send_metrics(&REGISTRY, app).await
}

fn init_prometheus() {
    // Register all metrics
    REGISTRY
        .register(Box::new(REQUEST_COUNTER.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(REQUEST_DURATION.clone()))
        .unwrap();
    REGISTRY.register(Box::new(ACTIVE_VUS.clone())).unwrap();
    REGISTRY.register(Box::new(TARGET_VUS.clone())).unwrap();
    REGISTRY.register(Box::new(SUCCESS_RATE.clone())).unwrap();
    REGISTRY
        .register(Box::new(REQUESTS_PER_SECOND.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(RESPONSE_TIME_P50.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(RESPONSE_TIME_P90.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(RESPONSE_TIME_P95.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(RESPONSE_TIME_P99.clone()))
        .unwrap();
    REGISTRY.register(Box::new(TEST_PHASE.clone())).unwrap();
    REGISTRY.register(Box::new(DATA_SENT.clone())).unwrap();
    REGISTRY.register(Box::new(DATA_RECEIVED.clone())).unwrap();

    // Initialize test phase
    TEST_PHASE.with_label_values(&["idle"]).set(1);
    TEST_PHASE.with_label_values(&["rampup"]).set(0);
    TEST_PHASE.with_label_values(&["hold"]).set(0);
    TEST_PHASE.with_label_values(&["rampdown"]).set(0);
}

// =============================================================================
// OUTPUT FUNCTIONS
// =============================================================================

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    const THRESHOLD: f64 = 1024.0;

    if bytes == 0 {
        return "0 B".to_string();
    }

    let bytes_f = bytes as f64;
    let unit_index = (bytes_f.log10() / THRESHOLD.log10()).floor() as usize;
    let unit_index = unit_index.min(UNITS.len() - 1);

    let size = bytes_f / THRESHOLD.powi(unit_index as i32);

    if size >= 100.0 {
        format!("{:.0} {}", size, UNITS[unit_index])
    } else if size >= 10.0 {
        format!("{:.1} {}", size, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

fn print_results(results: &TestResults) {
    println!("\n\nLoad Test Results");
    println!("═══════════════════════════════════════");
    println!("Total Requests:        {}", results.total_requests);
    println!(
        "Successful:            {} ({:.2}%)",
        results.successful_requests,
        (results.successful_requests as f64 / results.total_requests.max(1) as f64) * 100.0
    );
    println!(
        "Failed:                {} ({:.2}%)",
        results.failed_requests,
        (results.failed_requests as f64 / results.total_requests.max(1) as f64) * 100.0
    );
    println!("VUs:                   {}", results.vus);
    println!("Requests/sec:          {:.2}", results.requests_per_second);
    println!(
        "Test Duration:         {:.2}s",
        results.test_duration_seconds
    );

    println!("\nResponse Times (ms)");
    println!("───────────────────────────────────────");
    println!("Min:                   {:.2}", results.min_response_time_ms);
    println!("Max:                   {:.2}", results.max_response_time_ms);
    println!("Average:               {:.2}", results.avg_response_time_ms);
    println!("P50 (Median):          {:.2}", results.p50_response_time_ms);
    println!("P90:                   {:.2}", results.p90_response_time_ms);
    println!("P95:                   {:.2}", results.p95_response_time_ms);
    println!("P99:                   {:.2}", results.p99_response_time_ms);

    println!("\nNetwork Transfer");
    println!("───────────────────────────────────────");
    println!(
        "Total Data Sent:       {}",
        format_bytes(results.total_bytes_sent)
    );
    println!(
        "Total Data Received:   {}",
        format_bytes(results.total_bytes_received)
    );
    println!(
        "Total Data Transfer:   {}",
        format_bytes(results.total_bytes_sent + results.total_bytes_received)
    );
    if results.total_requests > 0 {
        println!(
            "Avg Sent per Request:  {}",
            format_bytes(results.total_bytes_sent / results.total_requests as u64)
        );
        println!(
            "Avg Received per Req:  {}",
            format_bytes(results.total_bytes_received / results.total_requests as u64)
        );
    }

    if !results.status_code_distribution.is_empty() {
        println!("\nStatus Code Distribution");
        println!("───────────────────────────────────────");
        let mut codes: Vec<_> = results.status_code_distribution.iter().collect();
        codes.sort_by_key(|&(code, _)| code);
        for (code, count) in codes {
            let percentage = (*count as f64 / results.total_requests.max(1) as f64) * 100.0;
            println!("{:3}: {:6} ({:5.2}%)", code, count, percentage);
        }
    }
    println!("═══════════════════════════════════════");
}

// =============================================================================
// MAIN FUNCTION
// =============================================================================

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize Prometheus if URL provided
    if args.prometheus_url.is_some() {
        init_prometheus();
    }

    // Build and run the load tester
    let tester = LoadTester::new(&args);
    let results = tester.run_load_test(&args).await;

    print_results(&results);

    // Save results to file if specified
    if let Some(output_path) = &args.output {
        match serde_json::to_string_pretty(&results) {
            Ok(json) => {
                if let Err(e) = std::fs::write(output_path, json) {
                    eprintln!("Failed to write results to file: {}", e);
                } else {
                    println!("\nResults saved to: {}", output_path);
                }
            }
            Err(e) => eprintln!("Failed to serialize results: {}", e),
        }
    }

    // Push final metrics if Prometheus is enabled
    if let Some(prometheus_url) = &args.prometheus_url {
        if let Err(e) = send_metrics_via_remote_write(prometheus_url, &args.app).await {
            eprintln!("Failed to push final metrics: {}", e);
        }
    }
}
