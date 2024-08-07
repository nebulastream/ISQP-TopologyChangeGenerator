use std::collections::{BTreeMap, HashMap};
use std::{fs, time};
use std::ops::Sub;
use std::time::Duration;
use geojson::GeoJson;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use crate::cell_data::{MultiTripAndCellData, RadioCell};
use serde_with::DurationMilliSeconds;
use crate::gtfs::{parse_duration, PartialBlock, Stop};

#[derive(Serialize, Deserialize, Debug)]
pub struct FixedTopology {
    //todo: check if we can just make that a tuple
    pub nodes: HashMap<u64, Vec<f64>>,
    pub slots: HashMap<u64, u16>,
    pub children: HashMap<u64, Vec<u64>>,
}


#[serde_as]
#[derive(Serialize, Deserialize, Clone)]
pub struct TopologyUpdate {
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    pub timestamp: time::Duration,
    #[serde(rename = "events")]
    pub events: Vec<ISQPEvent>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ISQPEvent {
    #[serde(rename = "parentId")]
    pub parent_id: u64,
    #[serde(rename = "childId")]
    pub child_id: u64,
    pub action: ISQPEventAction,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ISQPEventAction {
    add,
    remove,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SimulatedReconnects {
    initial_parents: Vec<(u64, u64)>,
    pub topology_updates: Vec<TopologyUpdate>,
}

impl SimulatedReconnects {
    ///create a geojson containing the first stop of all trips with the source groups having the
    /// same color
    // pub fn to_geojson(trips: &[PartialTrip], trip_to_node: &HashMap<String, u64>, node_to_source: HashMap<u64, u64>) -> GeoJson { let mut features = vec![];
    //     for trip in trips {
    //         let node_id = trip_to_node.get(&trip.trip_id).unwrap();
    //         let source_id = node_to_source.get(node_id).unwrap();
    //         let mut properties = geojson::JsonObject::new();
    //         let last_shap_point = trip.shape_points.iter().max_by(|).unwrap();
    //         properties.insert("marker-color".to_string(), serde_json::Value::String(color.to_string()));
    //         properties.insert("node id".to_string(), serde_json::Value::Number(serde_json::Number::from(*node_id)));
    //         properties.insert("source id".to_string(), serde_json::Value::Number(serde_json::Number::from(*source_id)));
    //         let feature = geojson::Feature {
    //             bbox: None,
    //             geometry: Some(geojson::Geometry::new(geojson::Value::Point(vec![stop.lon, stop.lat]))),
    //             id: None,
    //             properties: Some(properties),
    //             foreign_members: None,
    //         };
    //         features.push(feature);
    //     }
    //     GeoJson::FeatureCollection(geojson::FeatureCollection {
    //         bbox: None,
    //         features,
    //         foreign_members: None,
    //     })
    // }


    ///create a placement of logical sources by grouping the trips of a line into no overlapping
    ///groups of vehicles that directly follow each other on the track
    pub fn source_placement_from_trips(trips: &[PartialBlock], group_size: u16) -> HashMap<String, u64> {
        //create a vactor of references to the trips
        // let mut trip_refs: Vec<&PartialTrip> = trips.iter().collect();
        let mut trips: Vec<PartialBlock> = trips.iter().cloned().collect();
        //sort the trips by the start time
        for mut trip in trips.iter_mut() {
            trip.shape_points.sort_by(|a, b| a.time.cmp(&b.time));
        }
        trips.sort_by(|a, b| a.shape_points.first().unwrap().shape_pt_sequence.cmp(&b.shape_points.first().unwrap().shape_pt_sequence));

        //create a hashmap to store the mapping from source ids to trip ids
        let mut source_placement = HashMap::new();
        //iterate over the trips
        for (i, trip) in trips.iter().enumerate() {
            //get the source id
            let source_id = i as u64 / group_size as u64;
            //insert the source id and the trip id into the hashmap
            source_placement.insert(trip.block_id.clone(), source_id);
        }
        source_placement
    }
    pub fn from_topology_and_cell_data(topology: FixedTopology, mut cell_data: MultiTripAndCellData, cell_id_to_node_id: HashMap<(u64, u64), u64>, start_time: Duration, batch_interval: Option<Duration>, batch_gap: Option<Duration>, group_size: Option<u16>) -> (Self, HashMap<String, u64>,  Option<HashMap<u64, Vec<u64>>>) {
        let mut trip_to_node = HashMap::new();
        let mut initial_parents = vec![];
        let mut topology_update_map = BTreeMap::new();
        let mut child_id = topology.nodes.keys().max().unwrap() + 1;
        let _reconnect_count = 0;
        let batch_gap = Some(batch_gap.unwrap_or(Duration::from_secs(0)));

        cell_data.trips.sort_by(|a, b| a.trip.block_id.cmp(&b.trip.block_id));

        let mut source_placement_maps = match group_size {
            Some(group_size) => {
                let trip_vec: Vec<PartialBlock> = cell_data.trips.iter().map(|x| x.trip.clone()).collect();
                Some((SimulatedReconnects::source_placement_from_trips(&trip_vec, group_size), HashMap::new()))
            },
            None => None,
        };

        for mut trip in cell_data.trips {
            trip_to_node.insert(trip.trip.block_id.clone(), child_id);
            let mut current_batch_interval_start = None;
            let mut current_batch_timestamp = None;
            // let mut batch_gap = None;
            let mut batched_rem: Option<ISQPEvent> = None;
            let mut batched_add: Option<ISQPEvent> = None;

            // sort points by time
            trip.trip.shape_points.sort_by(|a, b| a.time.cmp(&b.time));

            if let Some(_) = batch_interval {
                current_batch_interval_start = Some(Duration::from_secs(0));
                current_batch_timestamp = Some(Duration::from_secs(0));
            };
            let point = trip.trip.shape_points.first().unwrap();
            let shape_id = point.shape_id.clone();
            let sequence_nr = point.shape_pt_sequence;

            let cell_id = trip.cell_data.get(&(shape_id, sequence_nr)).unwrap();
            let mut parent_id = *cell_id_to_node_id.get(cell_id).unwrap();
            initial_parents.push((parent_id, child_id));
            let mut previous_parent_id = parent_id;
            for point in &trip.trip.shape_points[1..] {
                let shape_id = point.shape_id.clone();
                let sequence_nr = point.shape_pt_sequence;

                let cell_id = trip.cell_data.get(&(shape_id, sequence_nr)).unwrap();
                parent_id = *cell_id_to_node_id.get(cell_id).unwrap();

                if parent_id != previous_parent_id {
                    //create topology update
                    let timestamp = point.time.expect("No time set for shape point").sub(start_time);

                    let add_event = ISQPEvent {
                        parent_id,
                        child_id,
                        action: ISQPEventAction::add,
                    };

                    let remove_event = ISQPEvent {
                        parent_id: previous_parent_id,
                        child_id,
                        action: ISQPEventAction::remove,
                    };

                    if let Some(interval) = batch_interval {
                        if timestamp < current_batch_interval_start.unwrap() + interval {
                            batched_rem = match batched_rem {
                                Some(b) => {
                                    Some(b.clone())
                                }
                                None => {
                                    Some(remove_event)
                                }
                            };
                            batched_add = Some(add_event);
                        } else {
                            //insert batch
                            if let Some(rem) = batched_rem {
                                let update_at_time = topology_update_map.entry(current_batch_timestamp.unwrap()).or_insert(TopologyUpdate { timestamp: current_batch_timestamp.unwrap(), events: vec![] });
                                update_at_time.events.push(rem);
                                update_at_time.events.push(batched_add.unwrap());
                            }
                            
                            //start new batch
                            batched_rem = Some(remove_event);
                            batched_add = Some(add_event);

                            //increment batch start time until we arrive at the batch containing the current time stamp
                            while timestamp > current_batch_interval_start.unwrap() + interval {
                                *current_batch_interval_start.as_mut().unwrap() += interval;
                                *current_batch_timestamp.as_mut().unwrap() += batch_gap.unwrap();
                            }
                        }
                    } else {
                        let update_at_time = topology_update_map.entry(timestamp).or_insert(TopologyUpdate { timestamp, events: vec![] });

                        update_at_time.events.push(ISQPEvent {
                            parent_id: previous_parent_id,
                            child_id,
                            action: ISQPEventAction::remove,
                        });

                        update_at_time.events.push(ISQPEvent {
                            parent_id,
                            child_id,
                            action: ISQPEventAction::add,
                        });
                    }
                }


                previous_parent_id = parent_id;
            }

            if let Some(rem) = batched_rem {
                let update_at_time = topology_update_map.entry(current_batch_timestamp.unwrap()).or_insert(TopologyUpdate { timestamp: current_batch_timestamp.unwrap(), events: vec![] });
                update_at_time.events.push(rem);
                update_at_time.events.push(batched_add.unwrap());
            }

            if let Some((trip_to_source, node_to_source)) = &mut source_placement_maps {
                let source_id = trip_to_source.get(&trip.trip.block_id).unwrap();
                node_to_source.insert(child_id, *source_id);
            }



            child_id += 1;
        }
        let source_mapping = if let Some((_, node_to_source)) = &source_placement_maps {
            // Some(node_to_source.clone())
            Some(node_to_source.clone().into_iter().map(|(k, v)| (k, vec![v])).collect())
        } else {
            None
        };

        (SimulatedReconnects { initial_parents, topology_updates: topology_update_map.into_values().collect() }, trip_to_node, source_mapping)
    }
}

impl FixedTopology {
    pub fn write_to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        // let json_string = std::fs::read_to_string(&self.paths.get_fixed_topology_nodes_path())?;
        // let topology: FixedTopology = serde_json::from_str(json_string.as_str())?;
        let json_string = serde_json::to_string_pretty(self)?;
        Ok(fs::write(path, json_string)?)
    }

    fn create_single_fog_layer(start_id: u64, default_resoucres: u16, radio_cells: &Vec<&RadioCell>) -> (FixedTopology, HashMap<(u64, u64), u64>) {
        let mut nodes = HashMap::new();
        let mut slots = HashMap::new();
        let mut children = HashMap::new();
        let mut cell_id_to_node_id = HashMap::new();

        //sort by id
        let mut radio_cells: Vec<&RadioCell> = radio_cells.iter().cloned().collect();
        radio_cells.sort_by(|a, b| a.id.cmp(&b.id));
        for (i, cell) in radio_cells.iter().enumerate() {
            let id = i as u64 + start_id;
            nodes.insert(id, vec![cell.lon, cell.lat]);
            slots.insert(id, default_resoucres);
            children.insert(id, vec![]);
            cell_id_to_node_id.insert((cell.id, cell.mnc), id);
        }
        (FixedTopology { nodes, slots, children }, cell_id_to_node_id)
    }
}

pub fn create_single_fog_layer_topology_from_cell_data(start_id: u64, default_resources: u16, cell_data: &MultiTripAndCellData) -> (FixedTopology, HashMap<(u64, u64), u64>) {
    FixedTopology::create_single_fog_layer(start_id, default_resources, &cell_data.radio_cells.values().collect())
}

