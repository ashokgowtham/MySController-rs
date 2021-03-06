pub mod adapter;

use crate::channel::{Receiver, Sender};
use crate::core::message::set::SetMessage;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use crate::model::node::Node;
use crate::model::sensor::Sensor;
use serde_json;
use std::sync::{Arc, RwLock, Weak};
use std::thread;

use webthing::server::ActionGenerator;
use webthing::{Action, Thing, ThingsType, WebThingServer};

struct Generator;

impl ActionGenerator for Generator {
    fn generate(
        &self,
        _thing: Weak<RwLock<Box<dyn Thing>>>,
        name: String,
        input: Option<&serde_json::Value>,
    ) -> Option<Box<dyn Action>> {
        let _input = match input {
            Some(v) => match v.as_object() {
                Some(o) => Some(o.clone()),
                None => None,
            },
            None => None,
        };

        let name: &str = &name;
        match name {
            _ => None,
        }
    }
}

fn get_things(
    pool: Pool<ConnectionManager<SqliteConnection>>,
    set_message_sender: Sender<SetMessage>,
) -> Vec<(Sensor, Arc<RwLock<Box<dyn Thing + 'static>>>)> {
    let mut sensor_list: Vec<Sensor> = vec![];
    let mut node_list: Vec<Node> = vec![];
    let mut things: Vec<(Sensor, Arc<RwLock<Box<dyn Thing + 'static>>>)> = Vec::new();

    {
        match pool.get() {
            Ok(conn) => {
                use crate::model::node::nodes::dsl::*;
                use crate::model::sensor::sensors::dsl::*;

                match nodes.load::<Node>(&conn) {
                    Ok(existing_nodes) => node_list = existing_nodes,
                    Err(e) => error!("Error while trying to get nodes {:?}", e),
                };
                match sensors.load::<Sensor>(&conn) {
                    Ok(existing_sensors) => sensor_list = existing_sensors,
                    Err(e) => error!("Error while trying to get sensors {:?}", e),
                };
            }
            Err(e) => error!("Error while trying to get db connection {:?}", e),
        }
    }
    for sensor in sensor_list {
        match (&node_list)
            .into_iter()
            .find(|node| node.node_id == sensor.node_id)
            .map(|node| node.node_name.clone())
        {
            Some(node_name) => {
                let thing = adapter::build_thing(
                    format!("{} - {}", node_name, sensor.sensor_type.thing_description())
                        .to_owned(),
                    sensor,
                    set_message_sender.clone(),
                );
                match thing {
                    Some(thing) => things.push(thing),
                    None => (),
                }
            }
            None => (),
        }
    }
    things
}

fn set_property(set_message: SetMessage, thing: &Arc<RwLock<Box<dyn Thing + 'static>>>) {
    info!("Received {:?}", &set_message);
    match set_message.value.to_json() {
        Some(new_value) => {
            // We don't need to set the property to things, we only need to publish it when we receive from sensors
            // {
            //     let mut t = thing.write().unwrap();
            //     match t.find_property(set_message.value.set_type.property_name()) {
            //         Some(prop) => {
            //             info!("Received {:?}", &set_message);
            //             // let _ = prop.set_value(new_value.clone());
            //         }
            //         None => warn!("No property found for {:?}", &set_message),
            //     };
            // }
            info!(
                "Notifying property {:?} with value {:?}",
                &set_message.value.set_type.property_name(),
                &new_value
            );
            thing
                .write()
                .unwrap()
                .property_notify(set_message.value.set_type.property_name(), new_value);
        }
        None => warn!("Unsupported set message {:?}", set_message),
    }
}

fn handle_sensor_outputs(
    things: &Vec<(Sensor, Arc<RwLock<Box<dyn Thing + 'static>>>)>,
    in_set_receiver: Receiver<SetMessage>,
) {
    loop {
        match in_set_receiver.recv() {
            Ok(set_message) => match things
                .into_iter()
                .find(|(sensor, _)| set_message.for_sensor(sensor))
                .map(|(_, thing)| thing)
            {
                Some(thing) => set_property(set_message, thing),
                None => warn!("No thing found matching {:?}", &set_message),
            },
            _ => (),
        }
    }
}

pub fn start_server(
    pool: Pool<ConnectionManager<SqliteConnection>>,
    set_message_sender: Sender<SetMessage>,
    in_set_receiver: Receiver<SetMessage>,
) {
    let things = get_things(pool, set_message_sender);
    let things_clone = things.clone();
    thread::spawn(move || {
        handle_sensor_outputs(&things_clone, in_set_receiver);
    });
    let things: Vec<Arc<RwLock<Box<dyn Thing + 'static>>>> =
        things.into_iter().map(|(_, thing)| thing).collect();
    thread::spawn(move || {
        if !things.is_empty() {
            let server = WebThingServer::new(
                ThingsType::Multiple(things, "MySensors".to_owned()),
                Some(8888),
                None,
                Box::new(Generator),
            );
            server.start();
        }
    });
}
