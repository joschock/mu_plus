#![no_std]

use core::{ffi::c_void, mem::size_of};
use r_efi::{efi, protocols};

//private unimplemented stub functions used to initialize the table.
extern "efiapi" fn raise_tpl_unimplemented(_: efi::Tpl) -> efi::Tpl {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn restore_tpl_unimplemented(_: efi::Tpl) {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn allocate_pages_unimplemented(
  _: efi::AllocateType,
  _: efi::MemoryType,
  _: usize,
  _: *mut efi::PhysicalAddress,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn free_pages_unimplemented(_: efi::PhysicalAddress, _: usize) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn get_memory_map_unimplemented(
  _: *mut usize,
  _: *mut efi::MemoryDescriptor,
  _: *mut usize,
  _: *mut usize,
  _: *mut u32,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn allocate_pool_unimplemented(_: efi::MemoryType, _: usize, _: *mut *mut c_void) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn free_pool_unimplemented(_: *mut c_void) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn create_event_unimplemented(
  _: u32,
  _: efi::Tpl,
  _: Option<efi::EventNotify>,
  _: *mut c_void,
  _: *mut efi::Event,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn set_timer_unimplemented(_: efi::Event, _: efi::TimerDelay, _: u64) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn wait_for_event_unimplemented(_: usize, _: *mut efi::Event, _: *mut usize) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn signal_event_unimplemented(_: efi::Event) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn close_event_unimplemented(_: efi::Event) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn check_event_unimplemented(_: efi::Event) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn install_protocol_interface_unimplemented(
  _: *mut efi::Handle,
  _: *mut efi::Guid,
  _: efi::InterfaceType,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn reinstall_protocol_interface_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: *mut c_void,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn uninstall_protocol_interface_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn handle_protocol_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: *mut *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn register_protocol_notify_unimplemented(
  _: *mut efi::Guid,
  _: efi::Event,
  _: *mut *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn locate_handle_unimplemented(
  _: efi::LocateSearchType,
  _: *mut efi::Guid,
  _: *mut c_void,
  _: *mut usize,
  _: *mut efi::Handle,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn locate_device_path_unimplemented(
  _: *mut efi::Guid,
  _: *mut *mut protocols::device_path::Protocol,
  _: *mut efi::Handle,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn install_configuration_table_unimplemented(_: *mut efi::Guid, _: *mut c_void) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn load_image_unimplemented(
  _: efi::Boolean,
  _: efi::Handle,
  _: *mut protocols::device_path::Protocol,
  _: *mut c_void,
  _: usize,
  _: *mut efi::Handle,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn start_image_unimplemented(_: efi::Handle, _: *mut usize, _: *mut *mut efi::Char16) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn exit_unimplemented(_: efi::Handle, _: efi::Status, _: usize, _: *mut efi::Char16) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn unload_image_unimplemented(_: efi::Handle) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn exit_boot_services_unimplemented(_: efi::Handle, _: usize) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn get_next_monotonic_count_unimplemented(_: *mut u64) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn stall_unimplemented(_: usize) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn set_watchdog_timer_unimplemented(_: usize, _: u64, _: usize, _: *mut efi::Char16) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn connect_controller_unimplemented(
  _: efi::Handle,
  _: *mut efi::Handle,
  _: *mut protocols::device_path::Protocol,
  _: efi::Boolean,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn disconnect_controller_unimplemented(_: efi::Handle, _: efi::Handle, _: efi::Handle) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn open_protocol_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: *mut *mut c_void,
  _: efi::Handle,
  _: efi::Handle,
  _: u32,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn close_protocol_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: efi::Handle,
  _: efi::Handle,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn open_protocol_information_unimplemented(
  _: efi::Handle,
  _: *mut efi::Guid,
  _: *mut *mut efi::OpenProtocolInformationEntry,
  _: *mut usize,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn protocols_per_handle_unimplemented(
  _: efi::Handle,
  _: *mut *mut *mut efi::Guid,
  _: *mut usize,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn locate_handle_buffer_unimplemented(
  _: efi::LocateSearchType,
  _: *mut efi::Guid,
  _: *mut c_void,
  _: *mut usize,
  _: *mut *mut efi::Handle,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn locate_protocol_unimplemented(
  _: *mut efi::Guid,
  _: *mut c_void,
  _: *mut *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn install_multiple_protocol_interfaces_unimplemented(
  _: *mut efi::Handle,
  _: *mut c_void,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn uninstall_multiple_protocol_interfaces_unimplemented(
  _: *mut efi::Handle,
  _: *mut c_void,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn calculate_crc32_unimplemented(_: *mut c_void, _: usize, _: *mut u32) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn copy_mem_unimplemented(_: *mut c_void, _: *mut c_void, _: usize) {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn set_mem_unimplemented(_: *mut c_void, _: usize, _: u8) {
  panic!("Call to unmocked boot services function.")
}

extern "efiapi" fn create_event_ex_unimplemented(
  _: u32,
  _: efi::Tpl,
  _: Option<efi::EventNotify>,
  _: *const c_void,
  _: *const efi::Guid,
  _: *mut efi::Event,
) -> efi::Status {
  panic!("Call to unmocked boot services function.")
}

pub fn mock_boot_services() -> efi::BootServices {
  efi::BootServices {
    hdr: efi::TableHeader {
      signature: efi::BOOT_SERVICES_SIGNATURE,
      revision: efi::BOOT_SERVICES_REVISION,
      header_size: size_of::<efi::BootServices>() as u32,
      crc32: 0,
      reserved: 0,
    },
    raise_tpl: raise_tpl_unimplemented,
    restore_tpl: restore_tpl_unimplemented,
    allocate_pages: allocate_pages_unimplemented,
    free_pages: free_pages_unimplemented,
    get_memory_map: get_memory_map_unimplemented,
    allocate_pool: allocate_pool_unimplemented,
    free_pool: free_pool_unimplemented,
    create_event: create_event_unimplemented,
    set_timer: set_timer_unimplemented,
    wait_for_event: wait_for_event_unimplemented,
    signal_event: signal_event_unimplemented,
    close_event: close_event_unimplemented,
    check_event: check_event_unimplemented,
    install_protocol_interface: install_protocol_interface_unimplemented,
    reinstall_protocol_interface: reinstall_protocol_interface_unimplemented,
    uninstall_protocol_interface: uninstall_protocol_interface_unimplemented,
    handle_protocol: handle_protocol_unimplemented,
    reserved: core::ptr::null_mut::<c_void>(),
    register_protocol_notify: register_protocol_notify_unimplemented,
    locate_handle: locate_handle_unimplemented,
    locate_device_path: locate_device_path_unimplemented,
    install_configuration_table: install_configuration_table_unimplemented,
    load_image: load_image_unimplemented,
    start_image: start_image_unimplemented,
    exit: exit_unimplemented,
    unload_image: unload_image_unimplemented,
    exit_boot_services: exit_boot_services_unimplemented,
    get_next_monotonic_count: get_next_monotonic_count_unimplemented,
    stall: stall_unimplemented,
    set_watchdog_timer: set_watchdog_timer_unimplemented,
    connect_controller: connect_controller_unimplemented,
    disconnect_controller: disconnect_controller_unimplemented,
    open_protocol: open_protocol_unimplemented,
    close_protocol: close_protocol_unimplemented,
    open_protocol_information: open_protocol_information_unimplemented,
    protocols_per_handle: protocols_per_handle_unimplemented,
    locate_handle_buffer: locate_handle_buffer_unimplemented,
    locate_protocol: locate_protocol_unimplemented,
    install_multiple_protocol_interfaces: install_multiple_protocol_interfaces_unimplemented,
    uninstall_multiple_protocol_interfaces: uninstall_multiple_protocol_interfaces_unimplemented,
    calculate_crc32: calculate_crc32_unimplemented,
    copy_mem: copy_mem_unimplemented,
    set_mem: set_mem_unimplemented,
    create_event_ex: create_event_ex_unimplemented,
  }
}

//private unimplemented stub functions used to initialize the table.
extern "efiapi" fn get_time_unimplemented(_: *mut efi::Time, _: *mut efi::TimeCapabilities) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn set_time_unimplemented(_: *mut efi::Time) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn get_wakeup_time_unimplemented(
  _: *mut efi::Boolean,
  _: *mut efi::Boolean,
  _: *mut efi::Time,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn set_wakeup_time_unimplemented(_: efi::Boolean, _: *mut efi::Time) -> efi::Status {
  unimplemented!()
}

extern "efiapi" fn set_virtual_address_map_unimplemented(
  _: usize,
  _: usize,
  _: u32,
  _: *mut efi::MemoryDescriptor,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn convert_pointer_unimplemented(_: usize, _: *mut *mut c_void) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn get_variable_unimplemented(
  _: *mut efi::Char16,
  _: *mut efi::Guid,
  _: *mut u32,
  _: *mut usize,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn get_next_variable_name_unimplemented(
  _: *mut usize,
  _: *mut efi::Char16,
  _: *mut efi::Guid,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn set_variable_unimplemented(
  _: *mut efi::Char16,
  _: *mut efi::Guid,
  _: u32,
  _: usize,
  _: *mut c_void,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn get_next_high_mono_count_unimplemented(_: *mut u32) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn reset_system_unimplemented(_: efi::ResetType, _: efi::Status, _: usize, _: *mut c_void) {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn update_capsule_unimplemented(
  _: *mut *mut efi::CapsuleHeader,
  _: usize,
  _: efi::PhysicalAddress,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn query_capsule_capabilities_unimplemented(
  _: *mut *mut efi::CapsuleHeader,
  _: usize,
  _: *mut u64,
  _: *mut efi::ResetType,
) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

extern "efiapi" fn query_variable_info_unimplemented(_: u32, _: *mut u64, _: *mut u64, _: *mut u64) -> efi::Status {
  panic!("Call to unmocked rutnime services function.")
}

pub fn mock_runtime_services() -> efi::RuntimeServices {
  efi::RuntimeServices {
    hdr: efi::TableHeader {
      signature: efi::RUNTIME_SERVICES_SIGNATURE,
      revision: efi::RUNTIME_SERVICES_REVISION,
      header_size: size_of::<efi::RuntimeServices>() as u32,
      crc32: 0,
      reserved: 0,
    },
    get_time: get_time_unimplemented,
    set_time: set_time_unimplemented,
    get_wakeup_time: get_wakeup_time_unimplemented,
    set_wakeup_time: set_wakeup_time_unimplemented,
    set_virtual_address_map: set_virtual_address_map_unimplemented,
    convert_pointer: convert_pointer_unimplemented,
    get_variable: get_variable_unimplemented,
    get_next_variable_name: get_next_variable_name_unimplemented,
    set_variable: set_variable_unimplemented,
    get_next_high_mono_count: get_next_high_mono_count_unimplemented,
    reset_system: reset_system_unimplemented,
    update_capsule: update_capsule_unimplemented,
    query_capsule_capabilities: query_capsule_capabilities_unimplemented,
    query_variable_info: query_variable_info_unimplemented,
  }
}

#[cfg(test)]
mod test {
  use crate::{get_time_unimplemented, mock_boot_services, mock_runtime_services, raise_tpl_unimplemented};

  #[test]
  fn mock_boot_services_should_produce_boot_services() {
    let boot_services = mock_boot_services();
    assert!(boot_services.raise_tpl == raise_tpl_unimplemented);
  }

  #[test]
  fn mock_runtime_services_should_produce_runtime_services() {
    let runtime_services = mock_runtime_services();
    assert!(runtime_services.get_time == get_time_unimplemented);
  }
}
