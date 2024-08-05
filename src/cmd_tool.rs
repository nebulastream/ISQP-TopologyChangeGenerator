use rusqlite::{Connection, named_params, Result};
use simulation_curator::gtfs;
use simulation_curator::cell_data;
use simulation_curator::nes_simulation;
use simulation_curator::nes_simulation::create_single_fog_layer_topology_from_cell_data;
use clap::Parser;
use simulation_curator::gtfs::{parse_duration, PartialTrip};

/// Program to generate topology change events
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the gtfs database
    #[arg(short, long, default_value = "gtfs_vbb.db")]
    db_path: String,

    ///  The time of the day from when the schedule needs to be selected.
    #[arg(short, long, default_value = "08:00:00")]
    start_time: String,

    /// The time of the day until when the schedule needs to be selected.
    #[arg(short, long, default_value = "09:00:00")]
    end_time: String,

    /// The day of the week (in number) for which the schedule needs to be selected. The week starts with 0 for Sunday and ends at 6 for Saturday.
    #[arg(long, default_value = "1")]
    day_of_the_week: String,

    /// The short names of lines for which the schedule needs to be extracted.
    #[arg(long, default_value = "S41")]
    line_name: String,

    /// The time interval in seconds that need to be represented by one second. This parameter allows us to speedup the time to increase the rate of topology changes.
    #[arg(long, default_value_t = 20)]
    batch_interval_size_in_seconds: u64,

    /// The frequency at which the batch of topology changes needs to be produced.
    #[arg(long, default_value_t = 4)]
    batch_frequency_in_seconds: u64,

    /// Path to the file where fixed_topology.json will be produced
    #[arg(long, default_value = "fixed_topology.json")]
    topology_path: String,

    /// Path to the file where topology_updates.json will be produced
    #[arg(long, default_value = "topology_updates.json")]
    topology_updates_path: String,

    /// Path to the file where geo.json will be produced
    #[arg(short, long, default_value = "geo.json")]
    geo_json_path: String,

    /// Name of the csv file containing OpenCelliD data.
    #[arg(short, long, default_value = "OpenCelliDGermanyData.csv")]
    open_cell_id_data_loc: String,

    /// The minimum number of measurements required for a cellular base station to be included in the experiment.
    #[arg(short, long, default_value_t = 10)]
    min_samples: u64,

    /// The type of network the base station supports. We use LTE only for our experiments.
    #[arg(short, long, default_value = "LTE")]
    radio: String,

    /// The amoount of physical sources corresponding to a single logical source
    #[arg(long, default_value = None)]
    source_group_size: Option<u16>,

    /// Path to the file where source_groups.json will be produced
    #[arg(long, default_value = "source_groups.json")]
    source_group_path: String,

    /// Path to the file where source_groups_geo.json will be produced
    #[arg(long, default_value = "source_groups_geo.json")]
    source_group_geo_path: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    //db
    let db = Connection::open(args.db_path)?;

    //time window
    let start_time = gtfs::parse_duration(&(args.start_time)).unwrap();
    let end_time = gtfs::parse_duration(&(args.end_time)).unwrap();

    // get routes and trips for a specific calender date
    let mut stmt = db.prepare("SELECT DISTINCT block_id \
                                                      FROM routes, trips, calendar_dates \
                                                      WHERE routes.route_id=trips.route_id \
                                                      AND trips.service_id=calendar_dates.service_id \
                                                      AND routes.route_short_name = :line_name \
                                                      AND strftime('%w',calendar_dates.date) = :day_of_the_week \
                                                      AND calendar_dates.date=( \
                                                            SELECT min(calendar_dates.date) \
                                                            FROM calendar_dates \
                                                            WHERE strftime('%w',calendar_dates.date) =:day_of_the_week)")?;


    let block_ids = stmt.query_map(named_params! {":line_name": args.line_name, ":day_of_the_week": args.day_of_the_week},
                                   |row| { Ok(row.get::<usize, String>(0)) })?;

    let mut partial_trips = Vec::new();
    // let mut all_shape_points = HashSet::new();
    for block_id in block_ids {
        if let Some(trip) = gtfs::read_stops_for_trip(block_id.unwrap().unwrap(), &db, start_time, end_time).unwrap() {
            partial_trips.push(trip);
        }
    }

    println!("partial trips: {}", partial_trips.len());

    // Find the cell towers used for connection
    let network_id = vec![2, 4, 9]; // For vodafone
    let beginning_of_2024 = 1704067200;
    let cells = cell_data::get_closest_cells_from_csv(&(args.open_cell_id_data_loc), &(args.radio), 262, &network_id, 0, beginning_of_2024, args.min_samples, &partial_trips);
    println!("cell towers {}", cells.radio_cells.len());
    let gj = cells.to_geojson();

    std::fs::write(args.geo_json_path, gj.to_string()).unwrap();

    //set default resources to max value
    let default_resources = u16::MAX;
    //create a topology and write it to json
    let (topology, cell_id_to_node_id) = create_single_fog_layer_topology_from_cell_data(2, default_resources, &cells);
    topology.write_to_file(&(args.topology_path)).unwrap();

    let batch_interval = std::time::Duration::from_secs(args.batch_interval_size_in_seconds);
    let batch_gap = std::time::Duration::from_secs(args.batch_frequency_in_seconds);
    let (simulated_reconnects, trip_to_node, source_groups) = nes_simulation::SimulatedReconnects::from_topology_and_cell_data(topology, cells, cell_id_to_node_id, start_time, batch_interval.into(), batch_gap.into(), args.source_group_size);
    println!("topology updates: {}", simulated_reconnects.topology_updates.len());
    let json_string = serde_json::to_string_pretty(&simulated_reconnects).unwrap();
    std::fs::write(args.topology_updates_path, json_string).unwrap();

    if let Some(source_groups) = source_groups {
        let json_string = serde_json::to_string_pretty(&source_groups).unwrap();
        std::fs::write(args.source_group_path, json_string).unwrap();

        //iterate over the trips and print the first stop and the corresponding source group
        for trip in partial_trips {
            // dbg!(&trip.stops);
            let node_id = trip_to_node.get(&trip.trip_id).unwrap();
            let source_group = source_groups.get(node_id).unwrap();
            if let Some(first_stop) = trip.stops.first() {
                println!("trip: {}, first stop departure: {}, first_stop_name: {} source group: {}", trip.trip_id, first_stop.departure_time, first_stop.stop_name, source_group);
            } else {
                println!("trip: {}, source group: {}", trip.trip_id, source_group);
            }
        }
    }

    Ok(())
}
