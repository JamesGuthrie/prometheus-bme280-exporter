use anyhow::{anyhow, Result};
use bme280::i2c::BME280;
use hyper::server::conn::Http;
use hyper::service::Service;
use hyper::{Body, Method, Request, Response, StatusCode};
use lazy_static::lazy_static;
use linux_embedded_hal::{Delay, I2CError, I2cdev};
use prometheus::{register_gauge, Encoder, Gauge, TextEncoder};
use tokio::net::TcpListener;

use bme280::Measurements;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

lazy_static! {
    static ref TEMPERATURE_GAUGE: Gauge = register_gauge!(
        "meter_temperature_celsius",
        "Ambient temperature in Celsius"
    )
    .unwrap();
    static ref PRESSURE_GAUGE: Gauge =
        register_gauge!("meter_pressure_pascals", "Atmospheric pressure in Pascals").unwrap();
    static ref HUMIDITY_GAUGE: Gauge =
        register_gauge!("meter_humidity_percent", "Relative humidity in %").unwrap();
}

const DEFAULT_DEV_PATH: &str = "/dev/i2c-1";

#[derive(Clone)]
struct TempServer {
    bme280: Arc<Mutex<BME280<I2cdev>>>,
}

impl TempServer {
    fn new() -> Result<TempServer> {
        let i2c_bus = I2cdev::new(DEFAULT_DEV_PATH)?;
        let mut bme280 = BME280::new_primary(i2c_bus);

        let mut delay = Delay;

        bme280
            .init(&mut delay)
            .map_err(|e| anyhow!("unable to init: {:?}", e))?;
        Ok(TempServer {
            bme280: Arc::new(Mutex::new(bme280)),
        })
    }

    fn measure(&self) -> Result<Measurements<I2CError>> {
        let mut delay = Delay;
        let measurement = self
            .bme280
            .lock()
            .map_err(|e| anyhow!("lock poisined: {:?}", e))?
            .measure(&mut delay)
            .map_err(|e| anyhow!("unable to measure: {:?}", e))?;
        Ok(measurement)
    }
}

impl Service<Request<Body>> for TempServer {
    type Response = Response<Body>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        match (req.method(), req.uri().path()) {
            (&Method::GET, "/metrics") => {
                let measurements = self.measure();

                if measurements.is_err() {
                    return Box::pin(async { Err(anyhow!("unable to measure")) });
                }
                let measurements = measurements.unwrap();

                TEMPERATURE_GAUGE.set(measurements.temperature.into());
                PRESSURE_GAUGE.set(measurements.pressure.into());
                HUMIDITY_GAUGE.set(measurements.humidity.into());

                let mut buffer = Vec::new();
                let encoder = TextEncoder::new();

                let metric_families = prometheus::gather();
                encoder
                    .encode(&metric_families, &mut buffer)
                    .expect("encoding failed");

                let buffer = buffer.clone();

                Box::pin(async { Ok(Response::builder().body(Body::from(buffer)).unwrap()) })
            }
            _ => Box::pin(async {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())
                    .unwrap())
            }),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], 3002));

    let server = TempServer::new()?;

    let listener = TcpListener::bind(addr).await?;
    println!("Listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;

        let server = server.clone();
        tokio::task::spawn(async {
            if let Err(err) = Http::new().serve_connection(stream, server).await {
                println!("Failed to serve connection: {:?}", err);
            }
        });
    }
}
