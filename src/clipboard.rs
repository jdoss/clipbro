use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::fd::AsFd;

use cosmic::cctk::{
    sctk::reexports::protocols_wlr::data_control::v1::client::{
        zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
        zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    },
    wayland_client::{
        self, Connection, Dispatch, EventQueue, Proxy,
        delegate_dispatch, event_created_child,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{
            wl_registry::WlRegistry,
            wl_seat::{self, WlSeat},
        },
    },
};

use crate::entry::MimeDataMap;

#[derive(thiserror::Error, Debug, Clone)]
pub enum ClipboardError {
    #[error("Wayland connection failed")]
    Connection,
    #[error("Wayland communication error")]
    Communication,
    #[error("Missing protocol: {0}")]
    MissingProtocol(&'static str),
    #[error("No seats available")]
    NoSeats,
    #[error("Clipboard empty")]
    Empty,
    #[error("Pipe error")]
    Pipe,
}

#[derive(Default)]
struct SeatData {
    name: Option<String>,
    device: Option<ZwlrDataControlDeviceV1>,
    offer: Option<ZwlrDataControlOfferV1>,
    primary_offer: Option<ZwlrDataControlOfferV1>,
}

impl SeatData {
    fn set_device(&mut self, device: Option<ZwlrDataControlDeviceV1>) {
        if let Some(old) = self.device.take() {
            old.destroy();
        }
        self.device = device;
    }

    fn set_offer(&mut self, offer: Option<ZwlrDataControlOfferV1>) {
        if let Some(old) = self.offer.take() {
            old.destroy();
        }
        self.offer = offer;
    }

    fn set_primary_offer(&mut self, offer: Option<ZwlrDataControlOfferV1>) {
        if let Some(old) = self.primary_offer.take() {
            old.destroy();
        }
        self.primary_offer = offer;
    }
}

struct WatcherState {
    seats: Vec<(WlSeat, SeatData)>,
    clipboard_manager: ZwlrDataControlManagerV1,
    offers: HashMap<ZwlrDataControlOfferV1, HashSet<String>>,
    got_primary_selection: bool,
}

impl WatcherState {
    fn seat_data_mut(&mut self, seat: &WlSeat) -> Option<&mut SeatData> {
        self.seats
            .iter_mut()
            .find(|e| &e.0 == seat)
            .map(|e| &mut e.1)
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for WatcherState {
    fn event(
        _: &mut Self, _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents, _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {}
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for WatcherState {
    fn event(
        _: &mut Self, _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _: &(), _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {}
}

impl Dispatch<WlSeat, ()> for WatcherState {
    fn event(
        state: &mut Self, seat: &WlSeat,
        event: <WlSeat as Proxy>::Event,
        _: &(), _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            if let Some(data) = state.seat_data_mut(seat) {
                data.name = Some(name);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, WlSeat> for WatcherState {
    fn event(
        state: &mut Self, _: &ZwlrDataControlDeviceV1,
        event: <ZwlrDataControlDeviceV1 as Proxy>::Event,
        seat: &WlSeat, _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id, HashSet::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(data) = state.seat_data_mut(seat) {
                    data.set_offer(id);
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                state.got_primary_selection = true;
                if let Some(data) = state.seat_data_mut(seat) {
                    data.set_primary_offer(id);
                }
            }
            zwlr_data_control_device_v1::Event::Finished => {
                if let Some(data) = state.seat_data_mut(seat) {
                    data.set_device(None);
                }
            }
            _ => {}
        }
    }

    event_created_child!(WatcherState, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE =>
            (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for WatcherState {
    fn event(
        state: &mut Self, offer: &ZwlrDataControlOfferV1,
        event: <ZwlrDataControlOfferV1 as Proxy>::Event,
        _: &(), _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            if let Some(mimes) = state.offers.get_mut(offer) {
                mimes.insert(mime_type);
            }
        }
    }
}

pub struct ClipboardWatcher {
    state: WatcherState,
    queue: EventQueue<WatcherState>,
}

impl ClipboardWatcher {
    pub fn new() -> Result<Self, ClipboardError> {
        let conn = Connection::connect_to_env()
            .map_err(|_| ClipboardError::Connection)?;

        let (globals, queue) = registry_queue_init::<WatcherState>(&conn)
            .map_err(|_| ClipboardError::Communication)?;

        let qh = &queue.handle();

        let clipboard_manager: ZwlrDataControlManagerV1 = globals
            .bind(qh, 1..=1, ())
            .map_err(|_| {
                ClipboardError::MissingProtocol("zwlr_data_control_manager_v1")
            })?;

        let registry = globals.registry();
        let seats: Vec<(WlSeat, SeatData)> = globals
            .contents()
            .with_list(|list| {
                list.iter()
                    .filter(|g| {
                        g.interface == WlSeat::interface().name
                            && g.version >= 2
                    })
                    .map(|g| {
                        let seat = registry.bind(g.name, 2, qh, ());
                        (seat, SeatData::default())
                    })
                    .collect()
            });

        if seats.is_empty() {
            return Err(ClipboardError::NoSeats);
        }

        let mut state = WatcherState {
            seats,
            clipboard_manager,
            offers: HashMap::new(),
            got_primary_selection: false,
        };

        for (seat, data) in &mut state.seats {
            let device = state.clipboard_manager.get_data_device(
                seat,
                qh,
                seat.clone(),
            );
            data.set_device(Some(device));
        }

        let mut watcher = Self { state, queue };

        watcher
            .queue
            .roundtrip(&mut watcher.state)
            .map_err(|_| ClipboardError::Communication)?;

        Ok(watcher)
    }

    pub fn watch_once(&mut self) -> Result<MimeDataMap, ClipboardError> {
        self.queue
            .blocking_dispatch(&mut self.state)
            .map_err(|_| ClipboardError::Communication)?;

        let offer = self
            .state
            .seats
            .first()
            .and_then(|(_, data)| data.offer.clone());

        let Some(offer) = offer else {
            return Err(ClipboardError::Empty);
        };

        let mime_types = self
            .state
            .offers
            .remove(&offer)
            .unwrap_or_default();

        tracing::debug!(
            "watch_once: offer exists, {} mime types, {} pending offers",
            mime_types.len(),
            self.state.offers.len()
        );

        if mime_types.is_empty() {
            return Err(ClipboardError::Empty);
        }

        let mut pipes = Vec::new();

        for mime_type in &mime_types {
            let (read, write) = std::io::pipe()
                .map_err(|_| ClipboardError::Pipe)?;

            offer.receive(mime_type.clone(), write.as_fd());
            pipes.push((mime_type.clone(), read));
            drop(write);
        }

        self.queue
            .roundtrip(&mut self.state)
            .map_err(|_| ClipboardError::Communication)?;

        let mut data = MimeDataMap::new();

        for (mime_type, mut reader) in pipes {
            let mut contents = Vec::new();
            match reader.read_to_end(&mut contents) {
                Ok(len) if len > 0 => {
                    data.insert(mime_type, contents);
                }
                _ => {}
            }
        }

        if data.is_empty() {
            return Err(ClipboardError::Empty);
        }

        Ok(data)
    }
}

use cosmic::iced_futures::futures::Stream;

pub fn clipboard_stream() -> impl Stream<Item = MimeDataMap> {
    cosmic::iced::stream::channel(500, async |mut output| {
        use cosmic::iced::futures::SinkExt;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<MimeDataMap>(5);

        tokio::task::spawn_blocking(move || {
            let mut watcher = match ClipboardWatcher::new() {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("Clipboard watcher init failed: {e}");
                    return;
                }
            };

            tracing::info!("Clipboard watcher started");

            loop {
                match watcher.watch_once() {
                    Ok(data) => {
                        tracing::debug!("Sending clipboard data to channel");
                        if tx.blocking_send(data).is_err() {
                            break;
                        }
                    }
                    Err(ClipboardError::Empty) => {}
                    Err(e) => {
                        tracing::error!("Clipboard watch error: {e}");
                        break;
                    }
                }
            }
        });

        loop {
            match rx.recv().await {
                Some(data) => {
                    tracing::debug!("Channel received clipboard data");
                    output.send(data).await.unwrap();
                }
                None => {
                    std::future::pending::<()>().await;
                }
            }
        }
    })
}
