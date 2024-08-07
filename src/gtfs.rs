use geojson::{Feature, GeoJson, Geometry, Value};
use std::time::Duration;
use rusqlite::{Connection, named_params};
use std::collections::{BTreeMap};
use std::hash::{Hash, Hasher};
use geo::VincentyDistance;
use crate::colors;

#[derive(Clone, Debug)]
pub struct PartialBlock {
    pub block_id: String,
    pub stops: Vec<Stop>,
    pub shape_points: Vec<ShapePoint>,
}

// get a vector of shape points from a vector of partial trips
pub fn get_shape_points_from_trips(trips: &[PartialBlock]) -> Vec<ShapePoint> {
    let mut shape_points = Vec::new();
    for trip in trips {
        for shape_point in &trip.shape_points {
            shape_points.push(shape_point.clone());
        }
    }
    shape_points
}

#[derive(Clone, Debug)]
pub struct Stop {
    trip_id: String,
    pub stop_id: String,
    pub stop_name: String,
    pub arrival_time: String,
    pub departure_time: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug)]
pub struct ShapePoint {
    pub shape_id: String,
    pub(crate) shape_pt_lat: f64,
    pub(crate) shape_pt_lon: f64,
    pub shape_pt_sequence: u64,
    pub time: Option<Duration>,
}

impl Eq for ShapePoint {}

impl PartialEq for ShapePoint {
    fn eq(&self, other: &Self) -> bool {
        self.shape_id == other.shape_id
    }
}

impl Hash for ShapePoint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.shape_id.hash(state);
    }
}

fn feature_line_from_shape_points(shape_points: &Vec<ShapePoint>) -> Feature {
    let mut properties = geojson::JsonObject::new();
    if !shape_points.is_empty() {
        properties.insert("shape_id".to_string(), serde_json::Value::String((&shape_points[0]).shape_id.clone()));
    } else {
        panic!("No shape points found for trip");
    }
    let mut coordinates = Vec::new();
    let mut shape_points = shape_points.clone();
    shape_points.sort_by_key(|x| x.time.unwrap());
    for shape_point in shape_points {
        coordinates.push(vec![shape_point.shape_pt_lon, shape_point.shape_pt_lat]);
    }
    Feature {
        bbox: None,
        geometry: Some(Geometry::new(Value::LineString(coordinates))),
        id: None,
        properties: Some(properties),
        foreign_members: None,
    }
}

impl ShapePoint {
    fn to_feature(&self) -> Feature {
        let mut properties = geojson::JsonObject::new();
        properties.insert("shape_id".to_string(), serde_json::Value::String(self.shape_id.clone()));
        properties.insert("shape_pt_sequence".to_string(), serde_json::Value::Number(serde_json::Number::from(self.shape_pt_sequence)));
        if let Some(time) = self.time {
            properties.insert("time".to_string(), serde_json::Value::String(duration_to_string(time)));
        }
        Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::Point(vec![self.shape_pt_lon, self.shape_pt_lat]))),
            id: None,
            properties: Some(properties),
            foreign_members: None,
        }
    }
}


//convert a vector of partial trips to geojson
pub fn to_geojson(trips: &Vec<PartialBlock>) -> GeoJson {
    let features = partial_trips_to_feature_collection(trips);
    let collection = geojson::FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };
    GeoJson::FeatureCollection(collection)
}

//todo: rename
pub fn partial_trips_to_feature_collection(trips: &Vec<PartialBlock>) -> Vec<Feature> {
    let mut stops = Vec::new();
    for trip in trips {
        for stop in &trip.stops {
            stops.push(stop);
        }
    }
    let mut features = Vec::new();
    let palette = colors::generate_color_palette(stops.len());
    for (i, stop) in stops.iter().enumerate() {
        features.push(stop.to_feature(Some(palette[i].as_str())));
    }


    for trip in trips {
        features.push(feature_line_from_shape_points(&trip.shape_points));
    }

    // for all shape points that have a time, add a point feature as well
    for trip in trips {
        for shape_point in &trip.shape_points {
            if let Some(_) = shape_point.time {
                features.push(shape_point.to_feature());
            }
        }
    }

    features
}

impl Stop {
    fn to_feature(&self, color: Option<&str>) -> Feature {
        let mut properties = geojson::JsonObject::new();
        properties.insert("stop_id".to_string(), serde_json::Value::String(self.stop_id.clone()));
        properties.insert("stop_name".to_string(), serde_json::Value::String(self.stop_name.clone()));
        properties.insert("arrival_time".to_string(), serde_json::Value::String(self.arrival_time.clone()));
        properties.insert("departure_time".to_string(), serde_json::Value::String(self.departure_time.clone()));
        //insert trip id
        properties.insert("trip_id".to_string(), serde_json::Value::String(self.trip_id.clone()));
        if let Some(color) = color {
            properties.insert("marker-color".to_string(), serde_json::Value::String(color.to_string()));
        }
        Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::Point(vec![self.lon, self.lat]))),
            id: None,
            // See the next section about Feature properties
            properties: Some(properties),
            foreign_members: None,
        }
    }
}

fn duration_to_string(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

pub fn parse_duration(time_str: &str) -> rusqlite::Result<Duration, Box<dyn std::error::Error>> {
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() != 3 {
        return Err("Invalid time format".into());
    }

    let hours: u64 = parts[0].parse()?;
    let minutes: u64 = parts[1].parse()?;
    let seconds: u64 = parts[2].parse()?;

    Ok(Duration::new(hours * 3600 + minutes * 60 + seconds, 0))
}

//read the stops for a trip
pub fn read_stops_for_block(block_id: String, db: &Connection, start_time: Duration, end_time: Duration) -> rusqlite::Result<Option<PartialBlock>, Box<dyn std::error::Error>> {
    println!("reading stops for block {}", block_id);
    let mut stmt = db.prepare("SELECT DISTINCT trip_id FROM trips WHERE trips.block_id=:block_id")?;
    let trip_ids = stmt.query_map(named_params! {":block_id": block_id}, |row| {
        Ok(row.get::<usize, String>(0))
    })?;

    let mut all_stops_in_range = vec![];
    let mut all_shape_points = vec![];
    for trip in trip_ids {
        let trip = trip??;
        let mut stmt = db.prepare("SELECT stops.stop_id, arrival_time, departure_time, stop_name FROM stop_times LEFT JOIN stops ON stops.stop_id=stop_times.stop_id WHERE trip_id=:trip_id ")?;

        let stop_times = stmt.query_map(named_params! {":trip_id": &trip}, |row| {
            Ok((row.get::<usize, String>(0), row.get::<usize, String>(1), row.get::<usize, String>(2), row.get::<usize, String>(3)))
        })?;

        let mut stops = BTreeMap::new();
        for stop_time in stop_times {
            let (stop_id, arrival_time, departure_time, stop_name) = stop_time.unwrap();
            let stop_id = stop_id.unwrap();
            let arrival_time = arrival_time.unwrap();
            let departure_time = departure_time.unwrap();
            let arrival_time = parse_duration(&arrival_time)?;
            let departure_time = parse_duration(&departure_time)?;
            let stop_name = stop_name.unwrap();
            // if arrival_time < start_time || departure_time > end_time {
            //     continue;
            // }
            let mut stmt = db.prepare("SELECT stop_lat, stop_lon FROM stops WHERE stop_id=:stop_id")?;
            let stop_coords = stmt.query_map(named_params! {":stop_id": stop_id}, |row| {
                Ok((row.get::<usize, f64>(0), row.get::<usize, f64>(1)))
            })?;
            for stop_coord in stop_coords {
                let (lat, lon) = stop_coord.unwrap();
                let lat = lat.unwrap();
                let lon = lon.unwrap();
                stops.insert(arrival_time, Stop {
                    trip_id: trip.clone(),
                    stop_id: stop_id.clone(),
                    arrival_time: duration_to_string(arrival_time),
                    departure_time: duration_to_string(departure_time),
                    lat,
                    lon,
                    stop_name: stop_name.clone(),
                });
            }
        }
        // println!("stops in list: {}", stops.len());

        // iterate over the collected stops and check if they are in the time range or preceed or succeed a stop in the range
        let mut stops = stops.into_iter().collect::<Vec<_>>();
        stops.sort_by_key(|(time, _)| *time);
        let mut stops_in_range = Vec::new();
        for i in 0..stops.len() {
            let (_, stop) = stops[i].clone();
            let arrival_time = parse_duration(&stop.arrival_time)?;
            let departure_time = parse_duration(&stop.departure_time)?;
            if departure_time >= start_time && arrival_time <= end_time {
                stops_in_range.push(stop);
            } else if i > 0 && i < stops.len() - 1 {
                let (_, prev_stop) = stops[i - 1].clone();
                let prev_time = parse_duration(&prev_stop.departure_time)?;
                let (_, next_stop) = stops[i + 1].clone();
                let next_time = parse_duration(&next_stop.arrival_time)?;
                if (prev_time < end_time && arrival_time > end_time) || (next_time > start_time && departure_time < start_time) {
                    stops_in_range.push(stop);
                }
            }
        }
        if stops_in_range.is_empty() {
            continue
        }

        let mut stmt = db.prepare("SELECT shape_id, shape_pt_lat, shape_pt_lon, shape_pt_sequence FROM shapes WHERE shape_id IN (SELECT shape_id FROM trips WHERE trip_id=:trip_id)")?;
        let shape_points = stmt.query_map(named_params! {":trip_id": &trip}, |row| {
            Ok(ShapePoint {
                shape_id: row.get::<usize, String>(0).unwrap(),
                shape_pt_lat: row.get::<usize, f64>(1).unwrap(),
                shape_pt_lon: row.get::<usize, f64>(2).unwrap(),
                shape_pt_sequence: row.get::<usize, u64>(3).unwrap(),
                time: None,
            })
        })?;
        let mut shape_points = shape_points.map(|x| x.unwrap()).collect::<Vec<_>>();
        //sort shape points by time
        shape_points.sort_by_key(|x| x.shape_pt_sequence);


        // store the shape points in a btree map indexed on the shape sequence number
        let mut shape_points_map = BTreeMap::new();
        for shape_point in &mut shape_points {
            shape_points_map.insert(shape_point.shape_pt_sequence, shape_point);
        }

        let mut first_index = None;
        let mut circular_end_time_tuple = None;

        // iterate over the stops in the range and try to find a matching shape point
        for (_, stop) in &stops {
            let mut closest_shape_point_sequence = None;
            let mut closest_distance = f64::MAX;
            for (_, shape_point) in shape_points_map.iter() {
                let stop_point = geo::Point::new(stop.lon, stop.lat);
                let shape_point_geo = geo::Point::new(shape_point.shape_pt_lon, shape_point.shape_pt_lat);
                let distance = shape_point_geo.vincenty_distance(&stop_point).unwrap();
                if distance < closest_distance {
                    closest_distance = distance;
                    closest_shape_point_sequence = Some(shape_point.shape_pt_sequence);
                }
            }
            if let Some(closest_shape_point) = closest_shape_point_sequence {
                let shape_point = shape_points_map.get_mut(&closest_shape_point).unwrap();
                let stop_center_time = (parse_duration(&stop.arrival_time).unwrap().as_millis() + parse_duration(&stop.departure_time).unwrap().as_millis()) / 2;
                if let Some(time) = shape_point.time {
                    circular_end_time_tuple = Some((time, Duration::from_millis(stop_center_time as u64)));
                }
                (*shape_point).time = Some(Duration::from_millis(stop_center_time as u64));
                if first_index.is_none() {
                    first_index = Some(shape_point.shape_pt_sequence);
                }
            }
        }

        //iterate over shape points and interpolate times
        let mut last_time_index = None;

        for (i, shape_point) in shape_points.clone().iter().enumerate() {
            //when a shape point with a time is found, start counting the points until the next time
            if let Some(time) = shape_point.time {
                if let Some(last_time_index) = last_time_index {
                    let last_time_elem: &ShapePoint = &shape_points[last_time_index];
                    let mut last_time = last_time_elem.time.unwrap();
                    let next_time = time;
                    let time_diff = if let Some(time_diff) = next_time.checked_sub(last_time) {
                        time_diff
                    } else {
                        let (first_time, sec_time): (Duration, Duration) = circular_end_time_tuple.expect("No circular end time");
                        assert_eq!(sec_time, last_time);
                        last_time = first_time;
                        next_time.checked_sub(last_time).unwrap()
                    };
                    assert!(time_diff > Duration::from_secs(0));
                    let num_points = i - last_time_index;
                    let time_diff_per_point = time_diff / num_points as u32;
                    for j in last_time_index + 1..i {
                        let time = last_time + time_diff_per_point * (j - last_time_index) as u32;
                        shape_points[j].time = Some(time);
                    }
                }
                last_time_index = Some(i);
            }
        }

        //wrap around
        if let Some((first_time, sec_time)) = circular_end_time_tuple {
            for (i, p) in shape_points.iter().enumerate().rev() {
                if let Some(time) = p.time {
                    for j in i + 1..shape_points.len() {
                        assert!(shape_points[j].time.is_none());
                        let time_diff = sec_time.checked_sub(time).unwrap();
                        let num_points = shape_points.len() - i;
                        let time_diff_per_point = time_diff / num_points as u32;
                        let time = first_time + time_diff_per_point * (j - i) as u32;
                        shape_points[j].time = Some(time);
                    }
                    break;
                }
            }
        }

        //filter out the shape points that are outside the time window
        shape_points.retain(|x| {
            if let Some(time) = x.time {
                time >= start_time && time <= end_time
            } else {
                false
            }
        });

        if shape_points.is_empty() {
            println!("No shape points in time window found for trip {}", block_id);
            continue
        }
        all_stops_in_range.append(&mut stops_in_range);
        all_shape_points.append(&mut shape_points);
    }
    if all_shape_points.is_empty() {
        println!("No shape points in time window found for trip {}", block_id);
        return Ok(None);
    }

    Ok(
        Some(
            PartialBlock {
                block_id: block_id,
                stops: all_stops_in_range,
                shape_points: all_shape_points,
            }
        )
    )
}