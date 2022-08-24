use anyhow::Result;
use bme280::i2c::BME280;
use hyper::server::conn::Http;
use hyper::service::Service;
use hyper::{Body, Method, Request, Response, StatusCode};
use lazy_static::lazy_static;
use linux_embedded_hal::{Delay, I2cdev};
use prometheus::{Encoder, Gauge, TextEncoder, register_gauge};
use tokio::net::TcpListener;

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};


lazy_static! {
    static ref TEMPERATURE_GAUGE: Gauge =
        register_gauge!("meter_temperature_celcius", "Ambient temperature in Celcius").unwrap();
}

lazy_static! {
    static ref PRESSURE_GAUGE: Gauge =
        register_gauge!("meter_pressure_pascals", "Atmospheric pressure in Pascals").unwrap();
}

lazy_static! {
    static ref HUMIDITY_GAUGE: Gauge =
        register_gauge!("meter_humidity_percent", "Relative humidity in %").unwrap();
}


#[derive(Clone)]
struct TempServer {
    bme280: Arc<Mutex<BME280<I2cdev>>>,
}

impl TempServer {
    fn new() -> Result<TempServer> {
        // using Linux I2C Bus #1 in this example
        let i2c_bus = I2cdev::new("/dev/i2c-1")?;
        // initialize the BME280 using the primary I2C address 0x76
        let mut bme280 = BME280::new_primary(i2c_bus);

        let mut delay = Delay;

        // initialize the sensor
        bme280.init(&mut delay).unwrap();
        Ok(TempServer {
            bme280: Arc::new(Mutex::new(bme280)),
        })
    }
}

impl Service<Request<Body>> for TempServer {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        match (req.method(), req.uri().path()) {
            (&Method::GET, "/metrics") => {
                let mut delay = Delay;
                // measure temperature, pressure, and humidity
                let measurements = self.bme280.lock().unwrap().measure(&mut delay).unwrap();

                TEMPERATURE_GAUGE.set(measurements.temperature.into());
                PRESSURE_GAUGE.set(measurements.pressure.into());
                HUMIDITY_GAUGE.set(measurements.humidity.into());

                let mut buffer = Vec::new();
                let encoder = TextEncoder::new();

                // Gather the metrics.
                let metric_families = prometheus::gather();
                // Encode them to send.
                encoder.encode(&metric_families, &mut buffer).unwrap();

                let output = String::from_utf8(buffer.clone()).unwrap();

                Box::pin(async { Ok(Response::builder().body(Body::from(output)).unwrap()) })
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

    let server = TempServer::new().unwrap();

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
