//! Prometheus Remote Write implementation

use prost::Message;
use reqwest::Client;
use snap::raw::Encoder;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

// Protobuf definitions for Prometheus Remote Write
#[derive(Clone, PartialEq, prost::Message)]
pub struct WriteRequest {
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<TimeSeries>,
    #[prost(message, repeated, tag = "2")]
    pub metadata: Vec<MetricMetadata>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TimeSeries {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<Sample>,
    #[prost(message, repeated, tag = "3")]
    pub exemplars: Vec<Exemplar>,
    #[prost(message, repeated, tag = "4")]
    pub histograms: Vec<Histogram>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Label {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Sample {
    #[prost(double, tag = "1")]
    pub value: f64,
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Exemplar {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    #[prost(double, tag = "2")]
    pub value: f64,
    #[prost(int64, tag = "3")]
    pub timestamp: i64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Histogram {
    #[prost(uint64, tag = "1")]
    pub count: u64,
    #[prost(double, tag = "2")]
    pub sum: f64,
    #[prost(message, repeated, tag = "3")]
    pub buckets: Vec<Bucket>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Bucket {
    #[prost(uint64, tag = "1")]
    pub cumulative_count: u64,
    #[prost(double, tag = "2")]
    pub upper_bound: f64,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct MetricMetadata {
    #[prost(string, tag = "1")]
    pub metric_name: String,
    #[prost(enumeration = "MetricType", tag = "2")]
    pub r#type: i32,
    #[prost(string, tag = "3")]
    pub help: String,
    #[prost(string, tag = "4")]
    pub unit: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum MetricType {
    Unknown = 0,
    Counter = 1,
    Gauge = 2,
    Histogram = 3,
    Gaugehistogram = 4,
    Summary = 5,
    Info = 6,
    Stateset = 7,
}

// Message for the metrics queue
#[derive(Debug)]
pub struct MetricsMessage {
    pub metric_families: Vec<prometheus::proto::MetricFamily>,
    pub app: String,
}

// Remote Write client with queue
pub struct RemoteWriteClient {
    client: Client,
    url: String,
    metrics_sender: Sender<MetricsMessage>,
    last_timestamp: Arc<Mutex<i64>>,
}

impl Clone for RemoteWriteClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            url: self.url.clone(),
            metrics_sender: self.metrics_sender.clone(),
            last_timestamp: self.last_timestamp.clone(),
        }
    }
}

impl RemoteWriteClient {
    pub fn new(url: String) -> Self {
        let client = Client::new();
        let (sender, receiver) = mpsc::channel();
        let last_timestamp = Arc::new(Mutex::new(0));

        // Spawn background thread for processing metrics
        let url_clone = url.clone();
        let client_clone = client.clone();
        let timestamp_clone = last_timestamp.clone();

        thread::spawn(move || {
            Self::metrics_processor_thread(receiver, client_clone, url_clone, timestamp_clone);
        });

        Self {
            client,
            url,
            metrics_sender: sender,
            last_timestamp,
        }
    }

    pub async fn send_metrics(
        &self,
        metrics: &prometheus::Registry,
        app: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let metric_families = metrics.gather();

        // Send metrics to the queue for sequential processing
        let message = MetricsMessage {
            metric_families,
            app: app.to_string(),
        };

        self.metrics_sender
            .send(message)
            .map_err(|e| format!("Failed to send metrics to queue: {}", e))?;

        Ok(())
    }

    // Background thread that processes metrics sequentially with monotonic timestamps
    fn metrics_processor_thread(
        receiver: Receiver<MetricsMessage>,
        client: Client,
        url: String,
        last_timestamp: Arc<Mutex<i64>>,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();

        while let Ok(message) = receiver.recv() {
            // Generate monotonic timestamp
            let timestamp = {
                let mut last = last_timestamp.lock().unwrap();
                let current = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;

                let timestamp = if current <= *last {
                    *last + 1 // Increment by 1ms if current time is not ahead
                } else {
                    current
                };

                *last = timestamp;
                timestamp
            };

            // Process metrics with the monotonic timestamp
            let timeseries =
                Self::process_metric_families(&message.metric_families, &message.app, timestamp);

            let write_request = WriteRequest {
                timeseries,
                metadata: Vec::new(),
            };

            // Send to Prometheus
            if let Err(e) = rt.block_on(Self::send_write_request_static(
                &client,
                &url,
                write_request,
            )) {
                eprintln!("Failed to send metrics via Remote Write: {}", e);
            }
        }
    }

    fn process_metric_families(
        metric_families: &[prometheus::proto::MetricFamily],
        app: &str,
        timestamp: i64,
    ) -> Vec<TimeSeries> {
        let mut timeseries = Vec::new();

        for family in metric_families {
            for metric in family.get_metric() {
                let base_labels = Self::create_base_labels(family.get_name(), app, metric);

                if metric.has_counter() {
                    timeseries.push(Self::create_counter_timeseries(
                        base_labels,
                        metric,
                        timestamp,
                    ));
                } else if metric.has_gauge() {
                    timeseries.push(Self::create_gauge_timeseries(
                        base_labels,
                        metric,
                        timestamp,
                    ));
                } else if metric.has_histogram() {
                    let mut hist_timeseries = Self::create_histogram_timeseries_simple(
                        base_labels,
                        family.get_name(),
                        metric,
                        timestamp,
                    );
                    timeseries.append(&mut hist_timeseries);
                }
            }
        }

        timeseries
    }

    fn create_base_labels(
        metric_name: &str,
        app: &str,
        metric: &prometheus::proto::Metric,
    ) -> Vec<Label> {
        let mut labels = vec![
            Label {
                name: "__name__".to_string(),
                value: metric_name.to_string(),
            },
            Label {
                name: "app".to_string(),
                value: app.to_string(),
            },
        ];

        for label_pair in metric.get_label() {
            labels.push(Label {
                name: label_pair.get_name().to_string(),
                value: label_pair.get_value().to_string(),
            });
        }

        labels
    }

    fn create_counter_timeseries(
        labels: Vec<Label>,
        metric: &prometheus::proto::Metric,
        timestamp: i64,
    ) -> TimeSeries {
        TimeSeries {
            labels,
            samples: vec![Sample {
                value: metric.get_counter().get_value(),
                timestamp,
            }],
            exemplars: Vec::new(),
            histograms: Vec::new(),
        }
    }

    fn create_gauge_timeseries(
        labels: Vec<Label>,
        metric: &prometheus::proto::Metric,
        timestamp: i64,
    ) -> TimeSeries {
        TimeSeries {
            labels,
            samples: vec![Sample {
                value: metric.get_gauge().get_value(),
                timestamp,
            }],
            exemplars: Vec::new(),
            histograms: Vec::new(),
        }
    }

    fn create_histogram_timeseries_simple(
        base_labels: Vec<Label>,
        metric_name: &str,
        metric: &prometheus::proto::Metric,
        timestamp: i64,
    ) -> Vec<TimeSeries> {
        let hist = metric.get_histogram();
        let mut timeseries = Vec::new();

        for bucket in hist.get_bucket() {
            let mut bucket_labels = base_labels.clone();
            bucket_labels.push(Label {
                name: "le".to_string(),
                value: bucket.get_upper_bound().to_string(),
            });

            timeseries.push(TimeSeries {
                labels: bucket_labels,
                samples: vec![Sample {
                    value: bucket.get_cumulative_count() as f64,
                    timestamp,
                }],
                exemplars: Vec::new(),
                histograms: Vec::new(),
            });
        }

        let mut count_labels = base_labels.clone();
        count_labels[0].value = format!("{}_count", metric_name);

        timeseries.push(TimeSeries {
            labels: count_labels,
            samples: vec![Sample {
                value: hist.get_sample_count() as f64,
                timestamp,
            }],
            exemplars: Vec::new(),
            histograms: Vec::new(),
        });

        let mut sum_labels = base_labels;
        sum_labels[0].value = format!("{}_sum", metric_name);

        timeseries.push(TimeSeries {
            labels: sum_labels,
            samples: vec![Sample {
                value: hist.get_sample_sum(),
                timestamp,
            }],
            exemplars: Vec::new(),
            histograms: Vec::new(),
        });

        timeseries
    }

    async fn send_write_request_static(
        client: &Client,
        url: &str,
        write_request: WriteRequest,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let encoded = write_request.encode_to_vec();

        let mut encoder = Encoder::new();
        let compressed = encoder
            .compress_vec(&encoded)
            .map_err(|e| format!("Failed to compress data: {}", e))?;

        let response = client
            .post(url)
            .header("Content-Type", "application/x-protobuf")
            .header("Content-Encoding", "snappy")
            .header("X-Prometheus-Remote-Write-Version", "0.1.0")
            .body(compressed)
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Remote write failed with status {}: {}", status, body).into());
        }

        Ok(())
    }
}
