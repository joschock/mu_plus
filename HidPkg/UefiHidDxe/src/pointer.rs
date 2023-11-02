use crate::hid_io::{HidReportReciever, HidIo};



pub struct PointerHidHandler {}

impl HidReportReciever for PointerHidHandler {
    fn initialize(&mut self, _hid_io: &dyn HidIo) -> Result<(),r_efi::efi::Status> {
        todo!()
    }
    fn receive_report(&mut self, _report: &[u8], _hid_io: &dyn HidIo) {
        todo!()
    }
    fn as_any(&mut self) ->  &mut dyn core::any::Any {
        todo!()
    }
}