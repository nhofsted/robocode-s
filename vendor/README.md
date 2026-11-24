# Patched dependencies

Crates here replace their crates.io release through `[patch.crates-io]` in the workspace
`Cargo.toml`. Each is the published source with a small change marked `robocode-s patch`.

## wgpu-hal 30.0.1

The Windows GUI died every day or so with egui-wgpu's "Failed to create staging buffer"
panic: wgpu had marked the device lost. The device-lost message was "Unexpected error
variant (driver implementation is at fault)": the NVIDIA Vulkan driver (RTX 2070 SUPER,
591.86) returned a `VkResult` that wgpu-hal doesn't map, wgpu-core turns any unmapped
result into device loss, and egui-wgpu can't recover from that. Polling the device right
after still succeeded, and it happened without sleep/wake or a driver reset.

The change, in `src/vulkan/mod.rs` and `src/vulkan/swapchain/native.rs`:

- `vkAcquireNextImageKHR` and `vkQueuePresentKHR` report a result that is neither device
  loss nor out of memory as `SurfaceError::Lost` (`map_swapchain_err`), not as device
  loss. Such a result is the swapchain's (wgpu's comments name
  `VK_ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT`, which it assumes can't happen), and
  egui-wgpu recovers from a lost surface by recreating it, in place.
- Every unexpected result, and `VK_ERROR_DEVICE_LOST`, is logged with its code and a
  backtrace (the GUI writes `log` warnings and errors to `robocode-s.log`), so a loss from
  anywhere else says where it came from.

To update: copy the new release from `~/.cargo/registry/src/*/wgpu-hal-<version>`, drop
`examples/`, `Cargo.toml.orig` and `.cargo_vcs_info.json`, and redo the marked changes, or
remove the patch if upstream handles these results.
