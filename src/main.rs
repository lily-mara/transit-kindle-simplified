use std::collections::HashMap;

use eyre::{ensure, eyre};

use chrono::prelude::*;
use reqwest::Client;
use serde::Deserialize;
use skia_safe::{
    utils::text_utils::Align, AlphaType, Bitmap, Canvas, Color4f, ColorType, Font, FontMgr,
    FontStyle, ImageInfo, Paint, Rect,
};

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::Response,
    routing::get,
    Router,
};
use tokio::net::TcpListener;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct StopMonitoringResponse {
    service_delivery: ServiceDelivery,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct ServiceDelivery {
    stop_monitoring_delivery: StopMonitoringDelivery,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct StopMonitoringDelivery {
    monitored_stop_visit: Vec<MonitoredStopVisit>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct MonitoredStopVisit {
    monitored_vehicle_journey: MonitoredVehicleJourney,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct MonitoredVehicleJourney {
    line_ref: Option<String>,
    direction_ref: Option<String>,
    destination_name: Option<String>,
    monitored_call: MonitoredCall,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct MonitoredCall {
    expected_arrival_time: Option<String>,
    stop_point_ref: String,
    destination_display: Option<String>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let app = Router::new().route("/stops.png", get(handle_stops_png));

    let listener = TcpListener::bind(&"0.0.0.0:3001").await?;

    eprintln!("Visit http://localhost:3001/stops.png");

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

async fn handle_stops_png() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/png")
        .body(Body::from(Bytes::from(get_image().await.unwrap())))
        .unwrap()
}

async fn get_image() -> eyre::Result<Vec<u8>> {
    let client = Client::new();

    let response_txt = client
        .get("http://api.511.org/transit/StopMonitoring?api_key=[your_key]&agency=SF")
        .send()
        .await?
        .text()
        .await?;

    let response: StopMonitoringResponse = serde_json::from_str(&response_txt)?;

    let mut journeys_i_care_about = Vec::new();

    for stop_visit in response
        .service_delivery
        .stop_monitoring_delivery
        .monitored_stop_visit
    {
        let stop = &stop_visit
            .monitored_vehicle_journey
            .monitored_call
            .stop_point_ref;
        if ["15419", "16996", "15692", "15696"].contains(&stop.as_ref()) {
            journeys_i_care_about.push(stop_visit.monitored_vehicle_journey);
        }
    }

    let mut directions_to_lines_destinations_to_journeys = HashMap::new();
    for journey in journeys_i_care_about {
        let Some(line) = journey.line_ref.clone() else {
            continue;
        };
        let Some(direction) = journey.direction_ref.clone() else {
            continue;
        };
        let Some(destination) = journey.monitored_call.destination_display.clone() else {
            continue;
        };

        directions_to_lines_destinations_to_journeys
            .entry(direction)
            .or_insert(HashMap::new())
            .entry((line, destination))
            .or_insert(Vec::new())
            .push(journey);
    }

    for lines_destinations_to_journeys in directions_to_lines_destinations_to_journeys.values_mut()
    {
        for journeys in lines_destinations_to_journeys.values_mut() {
            journeys.sort_by_key(|j| j.monitored_call.expected_arrival_time.clone());
        }
    }

    let png_bytes = draw_image(directions_to_lines_destinations_to_journeys)?;

    Ok(png_bytes)
}

fn text_bounds(text: &str, (x, y): (f32, f32), font: &Font, paint: &Paint) -> Rect {
    let (text_width, text_measurements) = font.measure_str(text, Some(paint));
    Rect::new(x, y + text_measurements.top, x + text_width, y)
}

fn draw_image(
    directions_to_lines_destinations_to_journeys: HashMap<
        String,
        HashMap<(String, String), Vec<MonitoredVehicleJourney>>,
    >,
) -> eyre::Result<Vec<u8>> {
    let mut bitmap = Bitmap::new();
    ensure!(bitmap.set_info(
        &ImageInfo::new((1024, 758), ColorType::Gray8, AlphaType::Unknown, None),
        None
    ));
    bitmap.alloc_pixels();

    let canvas = Canvas::from_bitmap(&bitmap, None).ok_or(eyre!("skia canvas"))?;

    canvas.clear(Color4f::new(1.0, 1.0, 1.0, 1.0));

    let font_manager = FontMgr::new();
    let typeface = font_manager
        .match_family_style("Arial", FontStyle::normal())
        .unwrap();
    let font = Font::new(typeface, 24.0);

    let black_paint = Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), None);
    let line_id_bubble_paint = Paint::new(Color4f::new(0.8, 0.8, 0.8, 1.0), None);

    let inbound_journeys = &directions_to_lines_destinations_to_journeys["IB"];
    let outbound_journeys = &directions_to_lines_destinations_to_journeys["OB"];

    let draw_times = |lines_destinations_to_journeys: &HashMap<
        (String, String),
        Vec<MonitoredVehicleJourney>,
    >,
                      x1: f32,
                      x2: f32| {
        let mut y = 60.0;
        for ((line_id, destination), journeys) in lines_destinations_to_journeys {
            let bounds = text_bounds(
                line_id,
                (x1 as f32 + 20.0, y as f32),
                &font,
                &line_id_bubble_paint,
            )
            .with_outset((8.0, 8.0));
            canvas.draw_round_rect(bounds, 24.0, 24.0, &line_id_bubble_paint);
            canvas.draw_str(line_id, (x1 + 20.0, y), &font, &black_paint);
            canvas.draw_str(destination, (bounds.right + 15.0, y), &font, &black_paint);

            let mut times_str = String::new();
            for journey in &journeys[..journeys.len().min(3)] {
                let Some(time_str) = &journey.monitored_call.expected_arrival_time else {
                    continue;
                };

                let Ok(time) = time_str.parse::<DateTime<Utc>>() else {
                    continue;
                };

                if time < Utc::now() {
                    continue;
                }

                let time = format!("{}, ", (time - Utc::now()).num_minutes());

                times_str.push_str(&time);
            }
            times_str.pop();
            times_str.pop();
            times_str.push_str(" min");

            canvas.draw_str_align(times_str, (x2 - 20.0, y), &font, &black_paint, Align::Right);
            canvas.draw_line((x1 + 10.0, y + 10.0), (x2 - 10.0, y + 10.0), &black_paint);
            y += 40.0;
        }
    };

    let width = 1024.0;
    let height = 758.0;
    let midpoint = 512.0;

    canvas.draw_rect(Rect::new(0.0, 0.0, width, 30.0), &line_id_bubble_paint);
    canvas.draw_str_align(
        "Muni Inbound",
        (midpoint / 2.0, 23.0),
        &font,
        &black_paint,
        Align::Center,
    );
    canvas.draw_str_align(
        "Muni Outbound",
        (midpoint + midpoint / 2.0, 23.0),
        &font,
        &black_paint,
        Align::Center,
    );
    canvas.draw_line((0.0, 30.0), (width, 30.0), &black_paint);

    draw_times(inbound_journeys, 0.0, midpoint);
    canvas.draw_line((midpoint, 0.0), (midpoint, height), &black_paint);
    draw_times(outbound_journeys, midpoint, width);

    let png = bitmap
        .as_image()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .ok_or(eyre!("skia image encode"))?;
    let png_bytes = png.as_bytes();

    Ok(png_bytes.to_owned())
}
