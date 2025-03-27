
This is a MQTT client based on [embassy](https://github.com/embassy-rs/embassy). I supports `no_std` without a `global_allocator`. For Testing it also supports `std` envoronments. 

# Features

If you want to add one of the missing features, please submit a pull request. 

## Protocol Versions

- [x] MQTT 3.1.1 Support
- [ ] MQTT 5 Support

## MQTT Features

- [x] Publish
- [x] Subscribe
  - [x] Auto Subscribe to topic when connection ist established
- [x] Unsubscribe
- [ ] Last Will
- [x] QoS 0
- [x] QoS 1
- [x] QoS 2
- [x] Automatically send ping

## Other features

- [ ] stable Rust support

# License

Copyright 2025 Siemers, Tim

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

