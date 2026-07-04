# TODO: MoonUI frame/present strictness

Status: deferred intentionally. This is a lower-level MoonUI/GPUI runtime task, not a local terminal widget bug. Do not mix it into small terminal hot-path fixes.

## Contract

- VBlank/frame-clock is only an opportunity to decide.
- If UI is not dirty and no `gpu_canvas` wants a frame, the window must not clear, draw, or present.
- If one chart wants a frame, this must not imply re-rendering or re-uploading the whole GPUI scene.
- Text and retained canvas layers should update only for the canvas/layer that actually changed.

## Current Gaps

1. Windows frame-clock still ticks all windows every vblank.
   - `moon-gpui-windows/src/platform.rs` iterates all HWNDs and posts `WM_GPUI_FRAME_CLOCK`.
   - `moon-gpui/src/platform.rs::set_gpu_canvas_active` exists, but the Windows backend does not use it as an active-window gate.
   - Needed: post frame-clock only to windows with dirty UI, active gpu canvas, explicit animation/direct manipulation, or another real reason.

2. GPU-only present in the DirectX renderer still calls `upload_scene_buffers(scene)`.
   - Native canvas frames can still re-upload ordinary GPUI scene buffers.
   - Needed: add scene revision/uploaded scene revision and skip scene-buffer upload when GPUI scene did not change.

3. One canvas present prepares text for every visible canvas in the window.
   - This was documented as a compromise in `GPU_CANVAS_TEXT_API.md` so neighbor labels would not disappear.
   - This violates the strict goal: one live chart must not force text rebuild for neighboring static charts.
   - Needed: keep per-canvas `wants_present`, retained text frames per canvas/layer, and prepare text only for `force_present || canvas_wants_present`.

4. Test guards/diagnostics are missing.
   - `scene_uploads/s` must not equal chart present rate on GPU-only frames.
   - `prepare_text/s` for static neighboring canvases must not grow because another canvas live-scrolls.
   - A static window without dirty UI/gpu canvas must not receive frame-clock every vblank.

## Non-Closure

Fixing `Shell`/`Orders` not to render at chart present rate only closes the top-level symptom. It does not prove:

- renderer scene uploads are skipped;
- neighboring canvas text is retained;
- static windows are not being frame-clocked.

