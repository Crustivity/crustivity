use std::{borrow::Cow, sync::Arc};

use crustivity_core::World;
use interaction::Init;
use vello::{
    kurbo::Affine,
    peniko::{Color, Fill},
    util::RenderContext,
    RendererOptions, Scene, SceneBuilder,
};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::EventLoop,
    window::WindowBuilder,
};

use pollster::FutureExt;

use crate::interaction::MouseClick;

pub mod interaction;

pub fn run(world: Arc<World>) {
    world.register_effect::<Scene>();

    let event_loop = EventLoop::<()>::new();

    let mut size = PhysicalSize::new(512, 512);
    let window = WindowBuilder::new()
        .with_inner_size(size)
        .build(&event_loop)
        .unwrap();

    let mut context = RenderContext::new().unwrap();
    let device_id = context.device(None).block_on().unwrap();
    let mut surface = context
        .create_surface(&window, size.width, size.width)
        .block_on()
        .unwrap();
    let mut renderer = {
        let device_handle = &mut context.devices[device_id];
        vello::Renderer::new(
            &device_handle.device,
            &RendererOptions {
                surface_format: Some(surface.format),
                timestamp_period: (&device_handle.queue).get_timestamp_period(),
            },
        )
        .unwrap()
    };

    world.emit_event(Init);
    let mut default_scene = Scene::new();

    let mut mouse_pos = PhysicalPosition::<f64>::new(0.0, 0.0);
    event_loop.run(move |event, _, control_flow| {
        control_flow.set_wait();

        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::Resized(s) => {
                    size = s;
                    context.resize_surface(&mut surface, size.width, size.height);
                    window.request_redraw();
                }
                WindowEvent::CloseRequested => control_flow.set_exit(),
                WindowEvent::CursorMoved { position, .. } => mouse_pos = position,
                WindowEvent::MouseInput { state, button, .. } => {
                    if state == ElementState::Released && button == MouseButton::Left {
                        world.emit_event(MouseClick(mouse_pos));
                    }
                }
                _ => (),
            },
            Event::MainEventsCleared => {
                if world.process_next_activation() {
                    window.request_redraw();
                }
            }
            Event::RedrawRequested(_) => {
                let device_handle = &context.devices[surface.dev_id];
                // let mut builder = SceneBuilder::for_scene(&mut scene);
                // use vello::kurbo::PathEl::*;
                // let shape = [
                //     MoveTo((0.0, 0.0).into()),
                //     LineTo((100.0, 0.0).into()),
                //     LineTo((100.0, 100.0).into()),
                //     ClosePath,
                // ];
                // builder.fill(
                //     Fill::NonZero,
                //     Affine::scale(2.0),
                //     Color::rgb(1.0, 1.0, 0.0),
                //     None,
                //     &shape,
                // );

                let mut scenes = world.receiv_effects::<Scene>();
                if let Some(scene) = scenes.pop() {
                    default_scene = scene;
                }

                let render_params = vello::RenderParams {
                    base_color: Color::WHITE,
                    width: size.width,
                    height: size.height,
                };

                let surface_texture = surface
                    .surface
                    .get_current_texture()
                    .expect("failed to get surface texture");

                _ = vello::block_on_wgpu(
                    &device_handle.device,
                    renderer.render_to_surface_async(
                        &device_handle.device,
                        &device_handle.queue,
                        &default_scene,
                        &surface_texture,
                        &render_params,
                    ),
                )
                .expect("failed to render to surface");

                surface_texture.present();
                device_handle.device.poll(wgpu::Maintain::Poll);
            }
            _ => (),
        }
    });
}
