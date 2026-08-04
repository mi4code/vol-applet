use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::env;

use gtk::gdk::{EventKey, SeatCapabilities};
use gtk::glib::idle_add_once;
use gtk::traits::{ContainerExt, ExpanderExt, GtkWindowExt, WidgetExt};
use gtk::{Application, ApplicationWindow, Inhibit};

use crate::audio::reload_outputs_in_popout;
use crate::audio::shared_output_list::{self, VolumeType};
use crate::elements::VolumeSlider;
use crate::options::OPTIONS;
use crate::tray_icon::TrayIcon;
use crate::{audio, AUDIO};

static POPOUT: Mutex<Option<Popout>> = Mutex::new(None);

pub struct Popout {
    pub container: gtk::Box,
    pub popout_menu: ApplicationWindow,
    pub sliders: HashMap<String, Box<VolumeSlider>>,
    ignore_next_callback: bool,
}
unsafe impl Sync for Popout {}
unsafe impl Send for Popout {}

fn is_wayland() -> bool {
    env::var("XDG_SESSION_TYPE").map(|v| v == "wayland").unwrap_or(false)
        || env::var("WAYLAND_DISPLAY").is_ok()
}

impl Popout {
    pub fn initialise(app: &Application) {
        let win = ApplicationWindow::builder()
            .application(app)
            .default_width(320)
            .default_height(50)
            .title("Volume")
            .type_hint(gtk::gdk::WindowTypeHint::PopupMenu)
            .decorated(false)
            .resizable(false)
            .build();

        let container = gtk::Box::builder()
            .margin(10)
            .spacing(6)
            .orientation(gtk::Orientation::Vertical)
            .build();

        win.set_child(Some(&container));

        win.connect_key_press_event(|_, e: &EventKey| -> Inhibit {
            if let Some(keycode) = e.keycode() {
                const ESC: u16 = 9;
                if keycode == ESC {
                    Popout::hide();
                }
            }
            gtk::Inhibit(false)
        });

        win.connect_button_press_event(|win, e| {
            let (x, y) = e.position();
            let (w, h) = win.size();
            if x < 0.0 || y < 0.0 || x > w as f64 || y > h as f64 {
                Popout::hide();
            }
            gtk::Inhibit(false)
        });

        win.connect_focus_in_event(|win, _| {
            grab_seat(&win.window().unwrap());
            gtk::Inhibit(false)
        });

        win.connect_focus_out_event(|_, _| {
            Popout::hide();
            gtk::Inhibit(false)
        });

        let popout = Self {
            container,
            popout_menu: win,
            sliders: HashMap::new(),
            ignore_next_callback: false,
        };

        POPOUT.lock().unwrap().replace(popout);
    }

    pub fn handle_callback(f: fn(&mut Popout)) {
        let mut a = POPOUT.lock().unwrap();
        let popout = a.as_mut().unwrap();
        if popout.ignore_next_callback {
            popout.ignore_next_callback = false;
            return;
        }
        f(popout);
    }

    fn set_geomerty(&mut self) {
        // If explicit placement was provided via --place, use that and skip
        // tray-icon geometry. On Wayland, prefer gtk-layer-shell for placement.
        if let Some((px, py)) = OPTIONS.place {
            if is_wayland() {
                // Initialize and use gtk-layer-shell to position the window on Wayland
                // This provides a reliable way to place the popup at exact coordinates.
                // Note: gtk-layer-shell must be available at build time (system library).
                #[allow(unused_imports)]
                use gtk_layer_shell::{Anchor, Edge, Layer};
                // Safe to ignore errors; if layer-shell calls fail we'll fallback to move_
                let _ = std::panic::catch_unwind(|| {
                    gtk_layer_shell::init();
                    gtk_layer_shell::init_for_window(&self.popout_menu);
                    // place on top layer
                    gtk_layer_shell::set_layer(&self.popout_menu, Layer::Top);
                    // anchor top and left, then set margins to px/py
                    gtk_layer_shell::set_anchor(&self.popout_menu, Edge::Top, true);
                    gtk_layer_shell::set_anchor(&self.popout_menu, Edge::Left, true);
                    gtk_layer_shell::set_margin(&self.popout_menu, Edge::Top, py);
                    gtk_layer_shell::set_margin(&self.popout_menu, Edge::Left, px);
                });
            } else {
                // X11: we can just move the window
                self.popout_menu.move_(px, py);
            }
            return;
        }

        // Default behavior: try to use tray icon geometry (on X11 or if provided by a tray bridge).
        self.popout_menu.set_size_request(320, 50);
        let (window_x, window_y) = self.popout_menu.position();
        let (window_width, window_height) = self.popout_menu.size();

        // Try to get tray geometry; TrayIcon::get_geometry() returns Option when not available
        let geom = TrayIcon::get_geometry();

        // If geometry is available, use it
        if let Some((icon, orientation)) = geom {
            let display = self.popout_menu.display();
            let monitor = display.monitor_at_point(window_x, window_y).unwrap();
            let monitor = monitor.geometry();

            #[allow(unused)]
            let mut x = 0;
            #[allow(unused)]
            let mut y = 0;

            if orientation == 1 {
                if icon.x + icon.width + window_width <= monitor.x() + monitor.width() {
                    x = icon.x + icon.width;
                } else {
                    x = icon.x - window_width;
                }
                if icon.y + window_height <= monitor.y() + monitor.height() {
                    y = icon.y;
                } else {
                    y = monitor.y() + monitor.height() - window_height;
                }
            } else {
                if icon.y + icon.height + window_height <= monitor.y() + monitor.height() {
                    y = icon.y + icon.height;
                } else {
                    y = icon.y - window_height;
                }
                if icon.x + window_width <= monitor.x() + monitor.width() {
                    x = icon.x;
                } else {
                    x = monitor.x() + monitor.width() - window_width;
                }
            }

            self.popout_menu.move_(x, y);
            return;
        }

        // Fallback: attempt to place near the pointer (useful on Wayland when tray geometry not available)
        let display = self.popout_menu.display();

        // Try to get pointer position via default seat
        if let Some(seat) = display.default_seat() {
            // Try pointer
            if let Some(pointer) = seat.pointer() {
                if let Ok(pos) = pointer.position() {
                    // position returns (f64,f64)
                    let (px, py) = pos;
                    let px = px as i32;
                    let py = py as i32;

                    // Attempt to ensure popup fits on monitor
                    if let Some(m) = display.monitor_at_point(px, py) {
                        let mon = m.geometry();
                        let mut x = px;
                        let mut y = py;
                        if x + window_width > mon.x() + mon.width() {
                            x = mon.x() + mon.width() - window_width;
                        }
                        if y + window_height > mon.y() + mon.height() {
                            y = mon.y() + mon.height() - window_height;
                        }
                        self.popout_menu.move_(x, y);
                        return;
                    } else {
                        self.popout_menu.move_(px, py);
                        return;
                    }
                }
            }
        }

        // Last resort: center on primary monitor
        let monitor = display.primary_monitor().unwrap();
        let mon = monitor.geometry();
        let x = mon.x() + (mon.width() - window_width) / 2;
        let y = mon.y() + (mon.height() - window_height) / 2;
        self.popout_menu.move_(x, y);
    }

    pub fn set_ignore_next_callback() {
        let mut a = POPOUT.lock().unwrap();
        let popout = a.as_mut().unwrap();
        popout.ignore_next_callback = true;
    }

    pub fn set_specific_volume(output_id: String, volume: f32) {
        idle_add_once(move || {
            let mut a = POPOUT.lock().unwrap();
            let popout = a.as_mut().unwrap();
            if let Some(output) = popout.sliders.get(&output_id) {
                output.set_volume_slider(volume);
            }
        });
    }

    pub fn set_specific_volume_label(output_id: String, volume: f32) {
        idle_add_once(move || {
            let mut a = POPOUT.lock().unwrap();
            let popout = a.as_mut().unwrap();
            if let Some(output) = popout.sliders.get(&output_id) {
                output.set_volume_label(volume);
            }
        });
    }

    pub fn set_specific_muted(output_id: String, muted: bool) {
        idle_add_once(move || {
            let mut a = POPOUT.lock().unwrap();
            let popout = a.as_mut().unwrap();
            if let Some(output) = popout.sliders.get(&output_id) {
                output.set_muted(muted);
            }
        });
    }

    pub fn update_outputs() {
        idle_add_once(|| {
            let mut a = POPOUT.lock().unwrap();
            let popout = a.as_mut().unwrap();
            let container = popout.container.clone();

            remove_child_widgets(popout);

            add_outputs_from_list(popout, container);

            popout.container.show_all();
        });

        if let Ok(output) = shared_output_list::get_default_output() {
            TrayIcon::set_muted(output.muted);
            TrayIcon::set_volume(output.volume);
        }
    }

    fn append_volume_slider(
        &self,
        container: &gtk::Box,
        output: audio::shared_output_list::Output,
        is_default: bool,
    ) -> VolumeSlider {
        let id = output.id.clone();
        let id_ = output.id.clone();
        VolumeSlider::new(
            container,
            Some(output.name),
            output.type_,
            output.icon_name,
            output.volume,
            output.muted,
            Rc::new(move |vol: f32| {
                handle_volume_slider_change(is_default, vol, id.clone());
            }),
            Rc::new(move || {
                handle_mute_button(id_.clone());
            }),
        )
    }

    pub fn show() {
        AUDIO.lock().unwrap().aud.get_outputs(Box::new(
            |outputs: Vec<shared_output_list::Output>| {
                reload_outputs_in_popout(outputs);
            },
        ));

        let mut a = POPOUT.lock().unwrap();
        let popout = a.as_mut().unwrap();

        popout.popout_menu.show();
        // popout.popout_menu.present();
        popout.set_geomerty();
    }

    pub fn hide() {
        let mut a = POPOUT.lock().unwrap();
        let popout = a.as_mut().unwrap();
        popout.popout_menu.hide();
        // ungrab(&popout.popout_menu.window().unwrap());
    }
}

fn add_outputs_from_list(popout: &mut Popout, container: gtk::Box) {
    let outputs = audio::shared_output_list::get_output_list();
    popout.sliders = HashMap::new();

    if outputs.is_empty() {
        popout
            .container
            .add(&gtk::Label::builder().label("No devices found.").build());
        return;
    }

    if OPTIONS.dont_group {
        for output in outputs {
            let is_default = output.is_default();
            popout.sliders.insert(
                output.id.clone(),
                Box::new(popout.append_volume_slider(&container, output, is_default)),
            );
        }
    } else {
        create_grouped(outputs, popout, container);
    }

    reposition_once_resized();
}

fn create_grouped(
    outputs: Vec<shared_output_list::Output>,
    popout: &mut Popout,
    container: gtk::Box,
) {
    let inputs = gtk::Expander::builder().label("Inputs").build();

    let inputs_container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(10)
        .build();

    inputs.add(&inputs_container);

    let streams = gtk::Expander::builder().label("Streams").build();

    let streams_container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(10)
        .build();

    streams.add(&streams_container);

    inputs.connect_expanded_notify(|_| {
        reposition_once_resized();
    });

    streams.connect_expanded_notify(|_| {
        reposition_once_resized();
    });

    for output in outputs {
        let id = output.id.clone();

        let slider = Box::new(match output.type_ {
            VolumeType::Sink => {
                let is_default = output.is_default();
                popout.append_volume_slider(&container, output, is_default)
            }
            VolumeType::Stream => popout.append_volume_slider(&streams_container, output, false),
            VolumeType::Input => popout.append_volume_slider(&inputs_container, output, false),
        });

        popout.sliders.insert(id, slider);
    }

    if OPTIONS.show_inputs {
        popout.container.add(&inputs);
    }

    if OPTIONS.show_streams {
        popout.container.add(&streams);
    }
}

fn reposition_once_resized() {
    // HACK: This is a hack to fix the issue where the popout doesn't resize
    //       for a little while.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut a = POPOUT.lock().unwrap();
        let popout = a.as_mut().unwrap();
        popout.set_geomerty();
    });
}

fn remove_child_widgets(popout: &mut Popout) {
    popout.container.foreach(|w| {
        popout.container.remove(w);
    });
}

fn handle_volume_slider_change(is_default: bool, vol: f32, id: String) {
    let vol = clamp_volume_to_percent(vol);

    if (vol - shared_output_list::get_stored_volume(&id)).abs() < 2. {
        return;
    }

    Popout::set_specific_volume_label(id.clone(), vol);

    if is_default {
        TrayIcon::set_volume(vol);
    }
    Popout::set_ignore_next_callback();

    let type_ = shared_output_list::type_of(&id);
    AUDIO.lock().unwrap().aud.set_volume(id, vol, type_);
}

fn clamp_volume_to_percent(vol: f32) -> f32 {
    if vol > 100. {
        100.
    } else if vol < 0. {
        0.
    } else {
        vol
    }
}

fn handle_mute_button(id: String) {
    let type_ = shared_output_list::type_of(&id);

    let mut muted = false;
    {
        let mut list = shared_output_list::OUTPUT_LIST.lock().unwrap();
        Popout::set_ignore_next_callback();
        for output in list.iter_mut() {
            if output.id == id {
                muted = !output.muted;
                output.muted = muted;
                if output.is_default() {
                    TrayIcon::set_muted(muted);
                }
                break;
            }
        }
    }

    Popout::set_specific_muted(id.clone(), muted);

    AUDIO.lock().unwrap().aud.set_muted(id, muted, type_);
}

fn grab_seat(popout: &gtk::gdk::Window) {
    let display = popout.display();
    let seat = display.default_seat().unwrap();

    let capabilities = gdk_sys::GDK_SEAT_CAPABILITY_POINTER;

    let status = seat.grab(
        popout,
        unsafe { SeatCapabilities::from_bits_unchecked(capabilities) },
        true,
        None,
        None,
        None,
    );

    if status != gtk::gdk::GrabStatus::Success {
        println!("Grab failed: {:?}", status);
    }
}

// fn ungrab(popout: &gtk::gdk::Window) {
//     let display = popout.display();
//     let seat = display.default_seat().unwrap();
//     seat.ungrab();
// }
