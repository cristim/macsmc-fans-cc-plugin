use crate::{
    SERVICE_ID, VERSION,
    device_service::v1::{
        CustomFunctionOneRequest, CustomFunctionOneResponse, EnableManualFanControlRequest,
        EnableManualFanControlResponse, FixedDutyRequest, FixedDutyResponse, HealthRequest,
        HealthResponse, InitializeDeviceRequest, InitializeDeviceResponse, LcdRequest, LcdResponse,
        LightingRequest, LightingResponse, ListDevicesRequest, ListDevicesResponse,
        ResetChannelRequest, ResetChannelResponse, ShutdownRequest, ShutdownResponse,
        SpeedProfileRequest, SpeedProfileResponse, StatusRequest, StatusResponse,
        device_service_server::DeviceService, health_response,
    },
    fan::Fan,
    models::{
        self,
        v1::{
            ChannelInfo, Device, DeviceInfo, SpeedOptions, channel_info::Options, status::FanSpeed,
        },
    },
};
use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
use sysinfo::Product;
use tokio::time::Instant;
use tonic::{Request, Response, Status};

pub struct FanService {
    start_time: Instant,
    fans: Arc<Vec<Fan>>,
    device: Device,
}

impl FanService {
    pub fn new(fans: Arc<Vec<Fan>>) -> Self {
        let mut channels = HashMap::new();

        for fan in fans.iter() {
            channels.insert(
                fan.id().to_string(),
                ChannelInfo {
                    label: Some(fan.label().into()),
                    options: Some(Options::SpeedOptions(SpeedOptions {
                        min_duty: fan.min_duty(),
                        max_duty: 100,
                        fixed_enabled: true,
                        extension: None,
                    })),
                },
            );
        }

        Self {
            start_time: Instant::now(),
            device: Device {
                id: SERVICE_ID.into(),
                name: Product::name().unwrap_or_else(|| SERVICE_ID.into()),
                uid_info: None,
                info: Some(DeviceInfo {
                    channels,
                    ..Default::default()
                }),
            },
            fans,
        }
    }

    fn fan(&self, channel_id: &str) -> Result<&Fan, Status> {
        self.fans
            .iter()
            .find(|fan| fan.id().to_string() == channel_id)
            .ok_or_else(|| Status::invalid_argument("Unknown channel ID"))
    }
}

#[tonic::async_trait]
impl DeviceService for FanService {
    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let reply = HealthResponse {
            name: SERVICE_ID.to_string(),
            version: VERSION.to_string(),
            status: health_response::Status::Ok.into(),
            uptime_seconds: self.start_time.elapsed().as_secs(),
        };
        Ok(Response::new(reply))
    }

    async fn list_devices(
        &self,
        _request: Request<ListDevicesRequest>,
    ) -> Result<Response<ListDevicesResponse>, Status> {
        Ok(Response::new(ListDevicesResponse {
            devices: vec![self.device.clone()],
        }))
    }

    async fn initialize_device(
        &self,
        _request: Request<InitializeDeviceRequest>,
    ) -> Result<Response<InitializeDeviceResponse>, Status> {
        Ok(Response::new(InitializeDeviceResponse {}))
    }

    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        crate::fan::release_all(&self.fans)?;

        Ok(Response::new(ShutdownResponse {}))
    }

    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        Ok(Response::new(StatusResponse {
            status: self
                .fans
                .iter()
                .map(|fan| models::v1::Status {
                    id: fan.id().to_string(),
                    metric: fan
                        .get_current_rpm()
                        .map(|rpm| {
                            models::v1::status::Metric::Speed(FanSpeed {
                                duty: Some(rpm as f64 / fan.max_rpm() as f64 * 100f64),
                                rpm: Some(rpm),
                            })
                        })
                        .ok(),
                })
                .collect(),
        }))
    }

    async fn reset_channel(
        &self,
        request: Request<ResetChannelRequest>,
    ) -> Result<Response<ResetChannelResponse>, Status> {
        self.fan(&request.get_ref().channel_id)?
            .release_to_firmware()?;

        Ok(Response::new(ResetChannelResponse {}))
    }

    async fn enable_manual_fan_control(
        &self,
        _request: Request<EnableManualFanControlRequest>,
    ) -> Result<Response<EnableManualFanControlResponse>, Status> {
        Ok(Response::new(EnableManualFanControlResponse {}))
    }

    async fn fixed_duty(
        &self,
        request: Request<FixedDutyRequest>,
    ) -> Result<Response<FixedDutyResponse>, Status> {
        let fan = self.fan(&request.get_ref().channel_id)?;
        fan.set_target_rpm(fan.duty_to_rpm(request.get_ref().duty))?;

        Ok(Response::new(FixedDutyResponse {}))
    }

    async fn speed_profile(
        &self,
        _request: Request<SpeedProfileRequest>,
    ) -> Result<Response<SpeedProfileResponse>, Status> {
        Err(Status::unimplemented("No Firmware Profiles"))
    }

    async fn lighting(
        &self,
        _request: Request<LightingRequest>,
    ) -> Result<Response<LightingResponse>, Status> {
        Err(Status::unimplemented("No Lighting Channels"))
    }

    async fn lcd(&self, _request: Request<LcdRequest>) -> Result<Response<LcdResponse>, Status> {
        Err(Status::unimplemented("No LCD Channels"))
    }

    async fn custom_function_one(
        &self,
        _request: Request<CustomFunctionOneRequest>,
    ) -> Result<Response<CustomFunctionOneResponse>, Status> {
        Err(Status::unimplemented("No Custom Function"))
    }
}
