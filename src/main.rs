extern crate lambda_runtime as lambda;
extern crate log;
extern crate simple_logger;
extern crate serde;
extern crate simple_error;
extern crate rusoto_ec2;
extern crate chrono;
extern crate futures;
extern crate tokio;

use lambda::{error::HandlerError, Context, lambda};
use log::{LevelFilter};
use serde::{Deserialize, Serialize};
use simple_logger::SimpleLogger;
use simple_error::bail;

use rusoto_core::{
    Region, HttpClient,credential::ChainProvider, credential::ProfileProvider, RusotoError
};
use rusoto_ec2::{
    Ec2Client, DescribeInstancesRequest, StopInstancesRequest, StartInstancesRequest, 
    Ec2, Filter, filter, StartInstancesResult, StopInstancesResult, StartInstancesError,
    StopInstancesError
};
use std::path::Path;
use chrono::prelude::*;
use futures::{future, Future};

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, PartialEq)]
struct VMInstance {
    instance_id: Option<String>,
    workSchedule: WorkTime,
    overtimeSchedule: Vec<Overtime>
}

#[derive(Debug, PartialEq)]
struct WorkTime {
    start: Option<NaiveTime>,
    end: Option<NaiveTime>,
    weekdays: Option<Vec<u32>>
}

#[derive(Debug, PartialEq)]
struct Overtime {
    start: Option<NaiveTime>,
    end: Option<NaiveTime>,
    date: Option<NaiveDate>
}

#[derive(Deserialize, Clone)]
struct CustomEvent {
    #[serde(rename = "firstName")]
    firstName: "name";
    
}

#[derive(Serialize, Clone)]
struct CustomOutput {
    started_instances: Vec<String>,
    stopped_instances: Vec<String>
}

fn ec2_client() -> Ec2Client {
    let profile_provider = ProfileProvider::with_configuration(Path::new("/home/laurence/.aws/credentials"), "dev2");
    println!("profile:{}, profile_path: {}", profile_provider.profile(), profile_provider.file_path().display());
    let chain = ChainProvider::with_profile_provider(profile_provider);
    Ec2Client::new_with(
        HttpClient::new().expect("failed to create request dispatcher"), 
        chain, 
        Region::ApEast1
    )
}

async fn get_instances(client: &Ec2Client, filters: Vec<Filter>) -> Vec<VMInstance>{

    let des_instance_req = DescribeInstancesRequest {
        filters: Some(filters),
        ..DescribeInstancesRequest::default()
    };

    let result; //= client.describe_instances(des_instance_req).await.unwrap();

   
    result = client.describe_instances(des_instance_req).await.unwrap();


    result.reservations.unwrap().iter().flat_map(|r| {
        r.instances.as_ref().unwrap().iter().map(|i| {
            let schedule_tag = i.tags.as_ref().unwrap().into_iter().filter(|tag| {tag.key == Some("schedule_workhours".to_string())}).next();
            let overtime_tag = i.tags.as_ref().unwrap().iter().filter(|tag| {tag.key == Some("schedule_extra_workhours".to_string())}).next();
            let instance_id = i.instance_id.clone();
            (instance_id, schedule_tag, overtime_tag)
        })
        .map(|instance_info| {
            VMInstance {
                //schedule_workhours: 1700-1300|0,1,2,3,4,5,6, 
                //schedule_extra_workhours: 1700-1300|17-09-2018,20-09-2018
                instance_id: instance_info.0,
                workSchedule: WorkTime {
                    start: {
                        NaiveTime::parse_from_str(
                            instance_info.1.unwrap().value.as_ref().unwrap().split("|").collect::<Vec<&str>>()[0].split("-").collect::<Vec<&str>>()[0], 
                            "%H%M"
                        ).ok()
                    },
                    end: {
                        NaiveTime::parse_from_str(
                            instance_info.1.unwrap().value.as_ref().unwrap().split("|").collect::<Vec<&str>>()[0].split("-").collect::<Vec<&str>>()[0], 
                            "%H%M"
                        ).ok()
                    },
                    weekdays: {
                        instance_info.1.unwrap().value.as_ref().unwrap().split("|")
                            .collect::<Vec<&str>>()[1].split(",").map(|wday| wday.parse().ok()).collect::<Option<Vec<u32>>>()
                    }
                },
                overtimeSchedule: {
                    instance_info.2.unwrap().value.as_ref().unwrap().split(",").map(|ot| {
                        Overtime {
                            start: {
                                NaiveTime::parse_from_str(
                                    ot.split("|").collect::<Vec<&str>>()[0].split("-").collect::<Vec<&str>>()[0], 
                                    "%H%M"
                                ).ok()
                            },
                            end: {
                                NaiveTime::parse_from_str(
                                    ot.split("|").collect::<Vec<&str>>()[0].split("-").collect::<Vec<&str>>()[1], 
                                    "%H%M"
                                ).ok()
                            },
                            date: {
                                NaiveDate::parse_from_str(ot.split("|").collect::<Vec<&str>>()[1], "%Y-%m-%d").ok()
                            }
                        }
                    }).collect()
                }
            }
        })
    }).collect::<Vec<VMInstance>>()
}

async fn start_instances(client: &Ec2Client, instances:Vec<VMInstance>) -> Result<StartInstancesResult, RusotoError<StartInstancesError>> {
    let start_instance_req = StartInstancesRequest {
        instance_ids: instances.into_iter().map(|i| i.instance_id.unwrap()).collect(),
        ..StartInstancesRequest::default()
    };
    client.start_instances(start_instance_req).await
}

async fn stop_instances(client: &Ec2Client, instances:Vec<VMInstance>) -> Result<StopInstancesResult, RusotoError<StopInstancesError>> {
    let stop_instances_req = StopInstancesRequest {
        instance_ids: instances.into_iter().map(|i| i.instance_id.unwrap()).collect(),
        ..StopInstancesRequest::default()
    };
    client.stop_instances(stop_instances_req).await
}

#[tokio::main]
async fn main() -> Result<(), Error> {

    SimpleLogger::new().with_level(LevelFilter::Debug).init().unwrap();
    lambda!(shutdown_sheduler);

    Ok(())
}

async fn shutdown_sheduler(e: CustomEvent, c: Context) -> Result<CustomOutput, HandlerError> {

    let ec2Client = ec2_client();
    let tag_key1 = filter!("tag:schedule_workhours");
    let tag_key2 = filter!("tag:schedule_extra_workhours");
    let instances = get_instances(&ec2Client, vec![tag_key1, tag_key2]).await;

    // time
    let now_nt = Utc::now().naive_utc();
    let now_hhmm_nt = NaiveTime::from_hms(now_nt.time().hour(), now_nt.time().minute(), 0);
    let date_nt = now_nt.date();
    let weekday_nt = now_nt.weekday();
    
    // instances to be startup
    let instances_to_startup = instances.into_iter().filter(|i| {
        i.overtimeSchedule.iter().any(|ot| ot.start.unwrap() == now_hhmm_nt && ot.date.unwrap() == date_nt) || 
        i.workSchedule.start.unwrap() == now_hhmm_nt && i.workSchedule.weekdays.unwrap().iter().any(|&d| d == weekday_nt.num_days_from_sunday())
    }).collect();
    let start_instances_result = start_instances(&ec2Client, instances_to_startup).await;

    // instances to be shutdown
    let instances_to_shutdown = instances.into_iter().filter(|i| {
        i.overtimeSchedule.iter().any(|ot| ot.end.unwrap() == now_hhmm_nt && ot.date.unwrap() == date_nt ) || 
        i.workSchedule.end.unwrap() == now_hhmm_nt && i.workSchedule.weekdays.unwrap().iter().any(|&d| d == weekday_nt.num_days_from_sunday())
    }).collect();
    let stop_instances_result = stop_instances(&ec2Client, instances_to_startup)

    Ok(CustomOutput{
        message: format!("Hello, {}!", e.firstname), 
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_startup_instances() {

    }

    #[test]
    fn get_shutdown_instances() {

    }
}