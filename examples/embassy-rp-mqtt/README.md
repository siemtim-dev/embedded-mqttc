# embassy-rp-mqtt example

This crate is an example showing how to use the `embedded-mqttc` crate on a Raspberry Pi Pico W to connect to an MQTT broker over Wi‑Fi. It demonstrates a minimal, practical integration of an embedded MQTT client on RP2040 hardware and can be used as a starting point for more complex applications.

## Features
- Connects the Pico W to a Wi‑Fi network
- Establishes an MQTT connection to a broker
- Publishes and subscribes to simple topics
- Uses `embedded-mqttc` APIs in an embedded, no-std context

## Prerequisites
- Raspberry Pi Pico W (or compatible RP2040 board with Wi‑Fi)
- Rust toolchain and cargo
- Cross-compile target for RP2040 (e.g. `thumbv6m-none-eabi`)
- Board flashing tool (probe-run, picotool, or UF2 drag-and-drop)
- Wi‑Fi SSID and password, and access to an MQTT broker (hostname/port, credentials if required)

## Quick start
1. Configure Wi‑Fi and broker credentials in the example (or via environment/config file as documented in the crate).
2. Build for the RP2040 target:
    - cargo build --release --target thumbv6m-none-eabi
3. Flash the firmware using your preferred workflow (probe-run, picotool, or UF2).
4. Monitor logs (serial) to verify Wi‑Fi and MQTT connection and to see published/subscribed messages.

## Notes
- This example focuses on demonstrating API usage and typical control flow; production use will require error handling, reconnection logic, and power/network management.
- Check the embedded-mqttc documentation for API details and supported features.

## License
See the crate root for license information.