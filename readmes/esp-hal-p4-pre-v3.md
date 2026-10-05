# esp-hal-p4-pre-v3

Compatibility build of [esp-hal](https://github.com/esp-rs/esp-hal) for
pre-v3 ESP32-P4 silicon (chip revision below 3.0).

If your chip is revision 3.0 or newer, use upstream `esp-hal` instead.
You do not need this crate.

```toml
[dependencies]
esp-hal = {
    package = "esp-hal-p4-pre-v3",
    version = "=1.1.0-p4v13.2",
    features = ["esp32p4"]
}
```

Rust code stays the same (`use esp_hal::...`). Validation scope and
release evidence are documented in the
[kit README](https://github.com/naoNao89/esp32p4-pre-v3-kit).
