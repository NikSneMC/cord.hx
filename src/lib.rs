//! Steel dylib (`libcord_hx`) implementing the Helix plugin.

mod plugin;

use steel::declare_module;
use steel::steel_vm::ffi::FFIModule;

declare_module!(plugin::module);
