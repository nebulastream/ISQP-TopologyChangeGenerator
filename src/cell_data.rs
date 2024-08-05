use std::collections::{HashMap};
use polars::prelude::*;
use polars_plan::plans::lit;
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use polars::datatypes::DataType;
use polars_plan::prelude::col;
use crate::geo_utils;
use crate::gtfs::{get_shape_points_from_trips, parse_duration, partial_trips_to_feature_collection, PartialTrip, ShapePoint};

//use polars to read a csv of cell data
pub fn read_cell_data_csv(file_path: &str) -> PolarsResult<DataFrame, > {
    let mut schema: Schema = Schema::new();
    schema.with_column("radio".parse().unwrap(), DataType::String);
    schema.with_column("mcc".parse().unwrap(), DataType::UInt64);
    schema.with_column("mnc".parse().unwrap(), DataType::UInt64);
    schema.with_column("tac".parse().unwrap(), DataType::UInt64);
    schema.with_column("cid".parse().unwrap(), DataType::UInt64);
    schema.with_column("unknown_col".parse().unwrap(), DataType::Int64);
    schema.with_column("lon".parse().unwrap(), DataType::Float64);
    schema.with_column("lat".parse().unwrap(), DataType::Float64);
    schema.with_column("range".parse().unwrap(), DataType::Float64);
    schema.with_column("samples".parse().unwrap(), DataType::UInt64);
    schema.with_column("changeable".parse().unwrap(), DataType::UInt32);
    schema.with_column("created".parse().unwrap(), DataType::UInt64);
    schema.with_column("updated".parse().unwrap(), DataType::UInt64);
    schema.with_column("average_signal".parse().unwrap(), DataType::UInt64);

    // read the csv file
    CsvReadOptions::default()
        .with_schema(Some(schema.into()))
        .with_has_header(false)
        .try_into_reader_with_file_path(Some(file_path.into()))?
        .finish()
}

//todo: move to additional sim_data module
#[derive(Debug, Clone)]
pub struct TripAndCellData {
    pub trip: PartialTrip,
    //todo: make typedefs for the identifiers
    pub cell_data: HashMap<(String, u64), (u64, u64)>,
}


#[derive(Debug)]
pub struct MultiTripAndCellData {
    pub trips: Vec<TripAndCellData>,
    pub radio_cells: HashMap<(u64, u64), RadioCell>,
}

impl MultiTripAndCellData {

    pub fn to_geojson(&self) -> GeoJson {
        let features = self.to_features();
        GeoJson::FeatureCollection(FeatureCollection {
            bbox: None,
            features,
            foreign_members: None,
        })
    }
    pub fn to_features(&self) -> Vec<Feature> {
        let trips = self.trips.iter().map(|t| t.trip.clone()).collect();
        let mut features = partial_trips_to_feature_collection(&trips);

        //iterate over the shape points of all trips and draw a line to the coordinates of the corresponding tower
        for trip in self.trips.iter() {
            for shape_point in trip.trip.shape_points.iter() {
                let mut line = vec![];
                let shape_id = &shape_point.shape_id;
                if let Some((tower_id, mnc)) = trip.cell_data.get(&(shape_id.clone(), shape_point.shape_pt_sequence)) {
                    if let Some(tower) = self.radio_cells.get(&(*tower_id, *mnc)) {
                        line.push(vec![tower.lon, tower.lat]);
                    }
                }
                // push the coordinates of the shape point
                line.push(vec![shape_point.shape_pt_lon, shape_point.shape_pt_lat]);
                let geometry = Value::LineString(line);
                let mut properties = geojson::JsonObject::new();
                properties.insert("stroke".to_string(), "#673AB7".into());
                properties.insert("stroke-width".to_string(), 2.into());
                features.push(geojson::Feature {
                    bbox: None,
                    geometry: Some(Geometry::new(geometry)),
                    id: None,
                    properties: Some(properties),
                    foreign_members: None,
                });
            }
        }

        for (_, tower) in self.radio_cells.iter() {
            features.push(tower.to_feature());
        }
        features
    }
}


pub fn get_closest_cells_from_csv(file_path: &str, radio: &str, mcc: u32, mncs: &Vec<u32>, start_time: u64, updated: u64, sample_count: u64, trips: &[PartialTrip]) -> MultiTripAndCellData {
    let mut towers = HashMap::new();

    // get the shape points from the list of trips
    let shape_points = get_shape_points_from_trips(trips);

    //read and filter cell data
    add_cell_data(file_path, radio, mcc, mncs, start_time, updated, sample_count, &shape_points, &mut towers);

    let mut trips_and_cells = vec![];
    for trip in trips {
        let mut shape_id_to_cell_id = HashMap::new();
        find_closest_towers(&shape_points, &mut towers, &mut shape_id_to_cell_id);
        let trip_and_cell_data = TripAndCellData {
            trip: trip.clone(),
            cell_data: shape_id_to_cell_id,
        };
        trips_and_cells.push(trip_and_cell_data);
    }
    //find closest cells for each shape point
    MultiTripAndCellData {
        trips: trips_and_cells,
        radio_cells: towers,
    }
}

// filter radio tower data by radio type, mcc, location and update time
pub fn filter_cell_data(df: &DataFrame, radio: &str, mcc: u32, mncs: &Vec<u32>, start_time: u64, updated: u64, sample_count: u64) -> PolarsResult<DataFrame> {
    let mnc_series = Series::new("mnc", mncs);
    df
        .clone()
        .lazy()
        .filter(
            col("radio").eq(lit(radio))
                .and(col("mcc").eq(lit(mcc)))
                .and(col("mnc").is_in(lit(mnc_series)))
                .and(col("created").gt(lit(start_time)))
                .and(col("updated").gt(lit(updated)))
                .and(col("samples").gt(lit(sample_count)))
        ).collect()
}

#[derive(Clone, Debug)]
pub struct RadioCell {
    pub lat: f64,
    pub lon: f64,
    pub id: u64,
    range: f64,
    pub mnc: u64,
}

impl RadioCell {
    fn to_feature(&self) -> geojson::Feature {
        let mut properties = geojson::JsonObject::new();
        properties.insert("id".to_string(), serde_json::Value::Number(self.id.into()));
        properties.insert("marker-color".to_string(), "#673AB7".into());
        properties.insert("range".to_string(), serde_json::Value::String(self.range.to_string()));
        geojson::Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::Point(vec![self.lon, self.lat]))),
            id: None,
            properties: Some(properties),
            foreign_members: None,
        }
    }
}

pub fn find_towers_in_range(df: DataFrame, shape_points: &Vec<ShapePoint>, towers: &mut HashMap<(u64, u64), RadioCell>) {
    for point in shape_points {
        if let Some(closest) = find_closest_tower(&df, point) {
            let tower_identifier = (closest.id, closest.mnc);
            towers.insert(tower_identifier, closest);
        }
    }
}

// read cell data, apply filters and find closest towers to a shape point
//todo: check which params can be reference instead of owned vals
pub fn find_closest_towers(shape_points: &Vec<ShapePoint>, towers: &mut HashMap<(u64, u64), RadioCell>, point_id_to_towers: &mut HashMap<(String, u64), (u64, u64)>) {
    for point in shape_points {
        let shape_point_identifier = (point.shape_id.clone(), point.shape_pt_sequence);
        if let Some(closest) = find_closest_tower_from_map(towers, point) {
            let tower_identifier = (closest.id, closest.mnc);
            point_id_to_towers.insert(shape_point_identifier, tower_identifier);
        }
    }
}

pub fn add_cell_data(file_path: &str, radio: &str, mcc: u32, mncs: &Vec<u32>, start_time: u64, updated: u64, sample_count: u64, shape_points: &Vec<ShapePoint>, towers: &mut HashMap<(u64, u64), RadioCell>) {
    let df = read_cell_data_csv(file_path).unwrap();
    let filtered = filter_cell_data(&df, radio, mcc, &mncs, start_time, updated, sample_count).unwrap();
    find_towers_in_range(filtered, shape_points, towers);
}

fn find_closest_tower_from_map(id_to_cell: &mut HashMap<(u64, u64), RadioCell>, point: &ShapePoint) -> Option<RadioCell> {
    //iterator over the rows
    let mut closest = None;
    let mut min_distance = f64::MAX;
    for (_, radio_cell) in id_to_cell.iter() {
        let distance = geo_utils::vincenty_dist_between_coordinates((radio_cell.lat, radio_cell.lon), (point.shape_pt_lat, point.shape_pt_lon));
        if distance < min_distance {
            min_distance = distance;
            closest = Some(radio_cell.clone());
        }
    }

    //check if the closest tower is within range
    if let Some(closest) = &closest {
        let distance = geo_utils::vincenty_dist_between_coordinates((closest.lat, closest.lon), (point.shape_pt_lat, point.shape_pt_lon));
        if distance > closest.range {
            println!("closest tower at distance {} but range is {}", distance, closest.range);
        }
    }

    closest
}

fn find_closest_tower(df: &DataFrame, point: &ShapePoint) -> Option<RadioCell> {
    //iterator over the rows
    let mut closest = None;
    let mut min_distance = f64::MAX;
    //todo: find more effiient way of iterating
    // https://users.rust-lang.org/t/using-for-loop-on-a-polars-dataframe/101819
    for i in 0..df.height() {
        let lat = df.column("lat").unwrap().f64().unwrap().get(i).unwrap();
        let lon = df.column("lon").unwrap().f64().unwrap().get(i).unwrap();
        let id = df.column("cid").unwrap().u64().unwrap().get(i).unwrap();
        let range = df.column("range").unwrap().f64().unwrap().get(i).unwrap();
        let mnc = df.column("mnc").unwrap().u64().unwrap().get(i).unwrap();
        //find the vincenty distance
        let distance = geo_utils::vincenty_dist_between_coordinates((lat, lon), (point.shape_pt_lat, point.shape_pt_lon));
        if distance < min_distance {
            min_distance = distance;
            closest = Some(RadioCell {
                lat,
                lon,
                id: id,
                range: range,
                mnc,
            });
        }
    }
    closest
}

// read and print a cell data csv
pub fn read_and_print_cell_data_csv(file_path: &str) -> PolarsResult<()> {
    let df = read_cell_data_csv(file_path)?;
    let vodafone_mncs = vec![2, 4, 9];
    let beginning_of_2024 = 1704067200;
    let min_samples = 100;
    let filtered = filter_cell_data(&df, "LTE", 262, &vodafone_mncs, 0, beginning_of_2024, min_samples)?;
    println!("{:?}", df);
    println!("{:?}", filtered);
    Ok(())
}
