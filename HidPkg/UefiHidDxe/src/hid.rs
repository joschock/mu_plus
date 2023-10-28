use crate::driver_binding::DriverBinding;

pub struct Hid {}

impl DriverBinding for Hid {
  fn driver_binding_supported(
      &self,
      _boot_services: &dyn rust_boot_services::UefiBootServices,
      _controller: r_efi::efi::Handle,
    ) -> r_efi::efi::Status {
      todo!()
  }
  fn driver_binding_start(
      &mut self,
      _boot_services: &dyn rust_boot_services::UefiBootServices,
      _controller: r_efi::efi::Handle,
    ) -> r_efi::efi::Status {
      todo!()
  }
  fn driver_binding_stop(
      &mut self,
      _boot_services: &dyn rust_boot_services::UefiBootServices,
      _controller: r_efi::efi::Handle,
    ) -> r_efi::efi::Status {
      todo!()
  }
}