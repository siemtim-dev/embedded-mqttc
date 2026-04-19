use cyw43::{Control, JoinOptions};
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use defmt::{error, info, panic, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_net::{Config, Runner, Stack, StackResources};
use embassy_rp::{
    Peri, bind_interrupts, clocks::RoscRng, gpio::{Level, Output}, peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO0}, pio::{InterruptHandler, Pio}
};
use embassy_time::Timer;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

pub struct WifiPins<'a> {
    pub pwr: Peri<'a, PIN_23>,
    pub cs: Peri<'a, PIN_25>,
    pub pio: Peri<'a, PIO0>,

    pub dio: Peri<'a, PIN_24>,
    pub clk: Peri<'a, PIN_29>,
    pub dma: Peri<'a, DMA_CH0>,
}

pub struct WifiParams<'a> {
    pub ssid: &'a str,
    pub key: Option<&'a str>,
}

impl<'a> WifiParams<'a> {

    async fn join(&self, control: &mut Control<'_>) {
        match self.key {
            Some(key) => self.join_wpa2(control, key).await,
            None => self.join_open(control).await,
        }
    }

    async fn join_wpa2(&self, control: &mut Control<'_>, key: &str) {
        let options = JoinOptions::new(key.as_bytes());

        loop {
            match control.join(self.ssid, options.clone()).await {
                Ok(_) => break,
                Err(err) => {
                    error!("join failed with status: {}", err.status);
                    Timer::after_secs(20).await;
                }
            }
        }
    }

    async fn join_open(&self, control: &mut Control<'_>) {
        let options = JoinOptions::new_open();

        loop {
            match control.join(self.ssid, options.clone()).await {
                Ok(_) => break,
                Err(err) => {
                    error!("join failed with status = {}", err.status);
                    Timer::after_secs(20).await;
                }
            }
        }
    }
}

// See https://github.com/embassy-rs/embassy/blob/891c5ee10584cd990dad529e3506fe1328e4e69d/examples/rp/src/bin/wifi_webrequest.rs
pub async fn wifi_connect<'a>(
    pins: WifiPins<'static>,
    params: &WifiParams<'a>,
    spawner: &Spawner,
) -> Stack<'static> {
    //let fw = include_bytes!("../firmware/cyw43-firmware/43439A0.bin");
    let fw = cyw43_firmware::CYW43_43439A0;

    let mut rng = RoscRng;

    let pwr = Output::new(pins.pwr, Level::Low);
    let cs = Output::new(pins.cs, Level::High);
    let mut pio = Pio::new(pins.pio, Irqs);
    
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        pins.dio,
        pins.clk,
        pins.dma,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());

    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(cyw43_task(runner)));

    // let clm = include_bytes!("../firmware/cyw43-firmware/43439A0_clm.bin");
    let clm = cyw43_firmware::CYW43_43439A0_CLM;
    control.init(clm).await;

    // Low power seems to produce more error messages of:
    // * WARN  failed to push rxd packet to the channel.
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // Use dhcp for ip addr
    let config = Config::dhcpv4(Default::default());

    static RESOURCES: StaticCell<StackResources<16>> = StaticCell::new();

    let seed = rng.next_u64();

    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    unwrap!(spawner.spawn(net_task(runner)));

    Timer::after_millis(500).await; // try to wait so that the net_tasks starts before joining

    info!("joining wifi network");
    params.join(&mut control).await;
    info!("joined wifi network");

    // Wait for DHCP, not necessary when using static IP
    /*
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    info!("waiting for link up...");
    while !stack.is_link_up() {
        Timer::after_millis(500).await;
    }
    info!("Link is up!");
    */

    info!("waiting for stack to be up...");

    match select(stack.wait_config_up(), Timer::after_secs(30)).await {
        embassy_futures::select::Either::First(()) => {
            info!("Stack is up!");
        },
        embassy_futures::select::Either::Second(()) => {
            panic!("dhcp timeout");
        },
    }

    if let Some(c) = stack.config_v4() {
        info!("Wifi Stack started with IPv4 {}", c.address.address())
    } else {
        warn!("IPv4 not configured")
    }

    if let Some(c) = stack.config_v6() {
        info!("Wifi Stack started with IPv6 {}", c.address.address())
    } else {
        warn!("IPv6 not configured")
    }

    return stack;
}

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    info!("starting cyw43_task");
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    info!("starting net_task");
    runner.run().await
}
