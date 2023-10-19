//! Key queue support for HID driver.
//!
//! This module manages a queue of pending keystrokes and current active keyboard state and provides support for
//! translating between HID usages and EFI keyboard primitives.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::{
  collections::{BTreeSet, VecDeque},
  vec::Vec,
};
use hidparser::report_data_types::Usage;
use hii_keyboard_layout::{EfiKey, HiiKey, HiiKeyDescriptor, HiiKeyboardLayout, HiiNsKeyDescriptor};
use r_efi::{
  efi,
  protocols::{self, hii_database::*, simple_text_input::InputKey, simple_text_input_ex::*},
};
use rust_advanced_logger_dxe::{debugln, DEBUG_WARN};

use crate::RUNTIME_SERVICES;

// The set of HID usages that represent modifier keys this driver is interested in.
#[rustfmt::skip]
const KEYBOARD_MODIFIERS: &[u16] = &[
  LEFT_CONTROL_MODIFIER, RIGHT_CONTROL_MODIFIER, LEFT_SHIFT_MODIFIER, RIGHT_SHIFT_MODIFIER, LEFT_ALT_MODIFIER,
  RIGHT_ALT_MODIFIER, LEFT_LOGO_MODIFIER, RIGHT_LOGO_MODIFIER, MENU_MODIFIER, PRINT_MODIFIER, SYS_REQUEST_MODIFIER,
  ALT_GR_MODIFIER];

// The set of HID usages that represent modifier keys that toggle state (as opposed to remain active while pressed).
const TOGGLE_MODIFIERS: &[u16] = &[NUM_LOCK_MODIFIER, CAPS_LOCK_MODIFIER, SCROLL_LOCK_MODIFIER];

// Control, Shift, and Alt modifiers.
const CTRL_MODIFIERS: &[u16] = &[LEFT_CONTROL_MODIFIER, RIGHT_CONTROL_MODIFIER];
const SHIFT_MODIFIERS: &[u16] = &[LEFT_SHIFT_MODIFIER, RIGHT_SHIFT_MODIFIER];
const ALT_MODIFIERS: &[u16] = &[LEFT_ALT_MODIFIER, RIGHT_ALT_MODIFIER];

// Defines whether a key stroke represents a key being pressed (KeyDown) or released (KeyUp)
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
  // Key is being pressed
  KeyUp,
  // Key is being released
  KeyDown,
}

// A wrapper for the KeyData type that allows definition of the Ord trait and aditional registration matching logic.
#[derive(Debug, Clone)]
pub(crate) struct OrdKeyData(pub protocols::simple_text_input_ex::KeyData);

impl Ord for OrdKeyData {
  fn cmp(&self, other: &Self) -> core::cmp::Ordering {
    let e = self.0.key.unicode_char.cmp(&other.0.key.unicode_char);
    if !e.is_eq() {
      return e;
    }
    let e = self.0.key.scan_code.cmp(&other.0.key.scan_code);
    if !e.is_eq() {
      return e;
    }
    let e = self.0.key_state.key_shift_state.cmp(&other.0.key_state.key_shift_state);
    if !e.is_eq() {
      return e;
    }
    return self.0.key_state.key_toggle_state.cmp(&other.0.key_state.key_toggle_state);
  }
}

impl PartialOrd for OrdKeyData {
  fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl PartialEq for OrdKeyData {
  fn eq(&self, other: &Self) -> bool {
    self.cmp(other).is_eq()
  }
}

impl Eq for OrdKeyData {}

impl OrdKeyData {
  // Returns whether this key matches the given registration. Note that this is not a straight compare - UEFI spec
  // allows for some degree of wildcard matching. Refer to UEFI spec 2.10 section 12.2.5.
  pub(crate) fn matches_registered_key(&self, registration: &Self) -> bool {
    // assign names here for brevity below.
    let self_char = self.0.key.unicode_char;
    let self_scan = self.0.key.scan_code;
    let self_shift = self.0.key_state.key_shift_state;
    let self_toggle = self.0.key_state.key_toggle_state;
    let register_char = registration.0.key.unicode_char;
    let register_scan = registration.0.key.scan_code;
    let register_shift = registration.0.key_state.key_shift_state;
    let register_toggle = registration.0.key_state.key_toggle_state;

    //char and scan must match (per the reference implementation in the EDK2 C code).
    if !(register_char == self_char && register_scan == self_scan) {
      return false;
    }

    //shift state must be zero or must match.
    if !(register_shift == 0 || register_shift == self_shift) {
      return false;
    }

    //toggle state must be zero or must match.
    if !(register_toggle == 0 || register_toggle == self_toggle) {
      return false;
    }
    return true;
  }
}

// This structure manages the queue of pending keystrokes
#[derive(Debug, Default)]
pub(crate) struct KeyQueue {
  layout: Option<HiiKeyboardLayout>,
  active_modifiers: BTreeSet<u16>,
  active_ns_key: Option<HiiNsKeyDescriptor>,
  partial_key_support_active: bool,
  key_queue: VecDeque<KeyData>,
  registered_keys: BTreeSet<OrdKeyData>,
  notified_key_queue: VecDeque<KeyData>,
}

impl KeyQueue {
  // resets the KeyQueue to initial state
  pub(crate) fn reset(&mut self) {
    self.active_modifiers.clear();
    self.active_ns_key = None;
    self.partial_key_support_active = false;
    self.key_queue.clear();
  }

  // Processes the given keystroke and updates the KeyQueue accordingly.
  pub(crate) fn keystroke(&mut self, key: Usage, action: KeyAction) {
    let Some(ref active_layout) = self.layout else {
      //nothing to do if no layout. This is unexpected: layout should be initialized with default if not present.
      debugln!(DEBUG_WARN, "Received keystroke without layout.");
      return;
    };

    let Some(efi_key) = usage_to_efi_key(key) else {
      //unsupported key usage, nothing to do.
      return;
    };

    // Check if it is a dependent key of a currently active "non-spacing" (ns) key.
    // Non-spacing key handling is described in UEFI spec 2.10 section 33.2.4.3.
    let mut current_descriptor: Option<HiiKeyDescriptor> = None;
    if let Some(ref ns_key) = self.active_ns_key {
      for descriptor in &ns_key.dependent_keys {
        if descriptor.key == efi_key {
          // found a dependent key for a previously active ns key.
          // de-activate the ns key and process the dependent descriptor.
          current_descriptor = Some(*descriptor);
          self.active_ns_key = None;
          break;
        }
      }
    }

    // If it is not a dependent key of a currently active ns key, then check if it is a regular or ns key.
    if current_descriptor.is_none() {
      for key in &active_layout.keys {
        match key {
          HiiKey::Key(descriptor) => {
            if descriptor.key == efi_key {
              current_descriptor = Some(*descriptor);
              break;
            }
          }
          HiiKey::NsKey(ns_descriptor) => {
            if ns_descriptor.descriptor.key == efi_key {
              // if it is an ns_key, set it as the active ns key, and no further processing is needed.
              self.active_ns_key = Some(ns_descriptor.clone());
              return;
            }
          }
        }
      }
    }

    if current_descriptor.is_none() {
      return; //could not find descriptor, nothing to do.
    }

    let current_descriptor = current_descriptor.unwrap();

    //handle modifiers that are active as long as they are pressed
    if KEYBOARD_MODIFIERS.contains(&current_descriptor.modifier) {
      match action {
        KeyAction::KeyUp => {
          self.active_modifiers.remove(&current_descriptor.modifier);
        }
        KeyAction::KeyDown => {
          self.active_modifiers.insert(current_descriptor.modifier);
        }
      }
    }

    //handle modifiers that toggle each time the key is pressed.
    if TOGGLE_MODIFIERS.contains(&current_descriptor.modifier) && action == KeyAction::KeyDown {
      if self.active_modifiers.contains(&current_descriptor.modifier) {
        self.active_modifiers.remove(&current_descriptor.modifier);
      } else {
        self.active_modifiers.insert(current_descriptor.modifier);
      }
    }

    //handle ctrl-alt-delete
    if CTRL_MODIFIERS.iter().any(|x| self.active_modifiers.contains(x))
      && ALT_MODIFIERS.iter().any(|x| self.active_modifiers.contains(x))
      && self.active_modifiers.contains(&DELETE_MODIFIER)
    {
      debugln!(DEBUG_WARN, "Ctrl-Alt-Del pressed, resetting system.");
      if let Some(runtime_services) = unsafe { RUNTIME_SERVICES.as_mut() } {
        (runtime_services.reset_system)(efi::RESET_WARM, efi::Status::SUCCESS, 0, core::ptr::null_mut());
      }
      panic!("Reset failed.");
    }

    if action == KeyAction::KeyUp {
      //nothing else to do.
      return;
    }

    // process the keystroke to construct a KeyData item to add to the queue.
    let mut key_data = protocols::simple_text_input_ex::KeyData {
      key: InputKey {
        unicode_char: current_descriptor.unicode,
        scan_code: modifier_to_scan(current_descriptor.modifier),
      },
      ..Default::default()
    };

    // retrieve relevant modifier state that may need to be applied to the key data.
    let shift_active = SHIFT_MODIFIERS.iter().any(|x| self.active_modifiers.contains(x));
    let alt_gr_active = self.active_modifiers.contains(&ALT_GR_MODIFIER);
    let caps_lock_active = self.active_modifiers.contains(&CAPS_LOCK_MODIFIER);
    let num_lock_active = self.active_modifiers.contains(&NUM_LOCK_MODIFIER);

    // Apply the shift modifier if needed. Track whether it was applied as the shift modifier is removed from the key
    // state if it was applied here.
    let mut shift_applied: bool = false;
    if (current_descriptor.affected_attribute & AFFECTED_BY_STANDARD_SHIFT) != 0 {
      if shift_active {
        //shift active
        if alt_gr_active {
          key_data.key.unicode_char = current_descriptor.shifted_alt_gr_unicode;
        } else {
          key_data.key.unicode_char = current_descriptor.shifted_unicode;
        }
        shift_applied = true;
      } else {
        //not shifted.
        if alt_gr_active {
          key_data.key.unicode_char = current_descriptor.alt_gr_unicode;
        }
      }
    }

    // if capslock is active, then invert the shift state of the key.
    if (current_descriptor.affected_attribute & AFFECTED_BY_CAPS_LOCK) != 0 && caps_lock_active {
      //Note: reference EDK2 implementation does not apply capslock to alt_gr.
      if key_data.key.unicode_char == current_descriptor.unicode {
        key_data.key.unicode_char = current_descriptor.shifted_unicode;
      } else if key_data.key.unicode_char == current_descriptor.shifted_unicode {
        key_data.key.unicode_char = current_descriptor.unicode;
      }
    }

    // for the num pad, numlock (and shift state) controls whether a number key or a control key (e.g. arrow) is queued.
    if (current_descriptor.affected_attribute & AFFECTED_BY_NUM_LOCK) != 0 {
      if num_lock_active && !shift_active {
        key_data.key.scan_code = SCAN_NULL;
      } else {
        key_data.key.unicode_char = 0x0000;
      }
    }

    //special handling for unicode 0x1B (ESC).
    if key_data.key.unicode_char == 0x01B && key_data.key.scan_code == SCAN_NULL {
      key_data.key.scan_code = SCAN_ESC;
      key_data.key.unicode_char = 0x0000;
    }

    if !self.partial_key_support_active && key_data.key.unicode_char == 0 && key_data.key.scan_code == SCAN_NULL {
      return; // no further processing required if there is no key or scancode and partial support is not active.
    }

    //initialize key state from active modifiers
    key_data.key_state = self.init_key_state();

    // if shift was applied above, then remove shift from key state. See UEFI spec 2.10 section 12.2.3.
    if shift_applied {
      key_data.key_state.key_shift_state &= !(LEFT_SHIFT_PRESSED | RIGHT_SHIFT_PRESSED);
    }

    // if a callback has been registered matching this key, enqueue it in the callback queue.
    if self.is_registered_key(key_data) {
      self.notified_key_queue.push_back(key_data);
    }

    // enqueue the key data.
    self.key_queue.push_back(key_data);
  }

  fn is_registered_key(&self, current_key: KeyData) -> bool {
    for registered_key in &self.registered_keys {
      if OrdKeyData(current_key).matches_registered_key(registered_key) {
        return true;
      }
    }
    return false;
  }

  // Creates a KeyState instance initialized based on the current modifier state.
  pub(crate) fn init_key_state(&self) -> KeyState {
    let mut key_state: KeyState = Default::default();

    key_state.key_shift_state = SHIFT_STATE_VALID;
    key_state.key_toggle_state = TOGGLE_STATE_VALID | KEY_STATE_EXPOSED;

    let key_shift_state = &mut key_state.key_shift_state;
    let key_toggle_state = &mut key_state.key_toggle_state;

    for modifier in &self.active_modifiers {
      match *modifier {
        LEFT_CONTROL_MODIFIER => *key_shift_state |= LEFT_CONTROL_PRESSED,
        RIGHT_CONTROL_MODIFIER => *key_shift_state |= RIGHT_CONTROL_PRESSED,
        LEFT_ALT_MODIFIER => *key_shift_state |= LEFT_ALT_PRESSED,
        RIGHT_ALT_MODIFIER => *key_shift_state |= RIGHT_ALT_PRESSED,
        LEFT_SHIFT_MODIFIER => *key_shift_state |= LEFT_SHIFT_PRESSED,
        RIGHT_SHIFT_MODIFIER => *key_shift_state |= RIGHT_SHIFT_PRESSED,
        LEFT_LOGO_MODIFIER => *key_shift_state |= LEFT_LOGO_PRESSED,
        RIGHT_LOGO_MODIFIER => *key_shift_state |= RIGHT_LOGO_PRESSED,
        MENU_MODIFIER => *key_shift_state |= MENU_KEY_PRESSED,
        SYS_REQUEST_MODIFIER | PRINT_MODIFIER => *key_shift_state |= SYS_REQ_PRESSED,
        SCROLL_LOCK_MODIFIER => *key_toggle_state |= SCROLL_LOCK_ACTIVE,
        NUM_LOCK_MODIFIER => *key_toggle_state |= NUM_LOCK_ACTIVE,
        CAPS_LOCK_MODIFIER => *key_toggle_state |= CAPS_LOCK_ACTIVE,
        _ => (),
      }
    }
    key_state
  }

  // pops and returns the front of the key queue
  pub(crate) fn pop_key(&mut self) -> Option<KeyData> {
    self.key_queue.pop_front()
  }

  // returns a copy of the key at the front of the queue
  pub(crate) fn peek_key(&self) -> Option<KeyData> {
    self.key_queue.front().cloned()
  }

  // pops and returns the front of the notify queue
  pub(crate) fn pop_notifiy_key(&mut self) -> Option<KeyData> {
    self.notified_key_queue.pop_front()
  }

  // returns a copy of the key at the front of the notify queue
  pub(crate) fn peek_notify_key(&self) -> Option<KeyData> {
    self.key_queue.front().cloned()
  }

  // set the key toggle state. This allows control of scroll/caps/num locks, as well as whether partial key state is
  // exposed.
  pub(crate) fn set_key_toggle_state(&mut self, toggle_state: KeyToggleState) {
    if (toggle_state & SCROLL_LOCK_ACTIVE) != 0 {
      self.active_modifiers.insert(SCROLL_LOCK_MODIFIER);
    } else {
      self.active_modifiers.remove(&SCROLL_LOCK_MODIFIER);
    }

    if (toggle_state & NUM_LOCK_ACTIVE) != 0 {
      self.active_modifiers.insert(NUM_LOCK_MODIFIER);
    } else {
      self.active_modifiers.remove(&NUM_LOCK_MODIFIER);
    }

    if (toggle_state & CAPS_LOCK_ACTIVE) != 0 {
      self.active_modifiers.insert(CAPS_LOCK_MODIFIER);
    } else {
      self.active_modifiers.remove(&CAPS_LOCK_MODIFIER);
    }

    self.partial_key_support_active = (toggle_state & KEY_STATE_EXPOSED) != 0;
  }

  // Returns a vector of HID usages corresponding to the active LEDs based on the active modifier state.
  pub(crate) fn get_active_leds(&self) -> Vec<Usage> {
    self.active_modifiers.iter().cloned().filter_map(|x| modifer_to_led_usage(x)).collect()
  }

  // Returns the current keyboard layout that the KeyQueue is using.
  pub(crate) fn get_layout(&self) -> Option<HiiKeyboardLayout> {
    self.layout.clone()
  }

  // Sets the current keyboard layout that the KeyQueue should use.
  pub(crate) fn set_layout(&mut self, new_layout: Option<HiiKeyboardLayout>) {
    self.layout = new_layout;
  }

  // Add a registration key for notifications; if a keystroke matches this key data, it will be added to the notify
  // queue in addition to the normal key queue.
  pub(crate) fn add_notify_key(&mut self, key_data: OrdKeyData) {
    self.registered_keys.insert(key_data);
  }

  // Remove a previously added notify key; keystrokes matching this key data will no longer be added to the notify
  // queue.
  pub(crate) fn remove_notify_key(&mut self, key_data: &OrdKeyData) {
    self.registered_keys.remove(key_data);
  }
}

// Helper routine that converts a HID Usage to the corresponding EfiKey.
fn usage_to_efi_key(usage: Usage) -> Option<EfiKey> {
  //Refer to UEFI spec version 2.10 figure 34.3
  match usage.into() {
    0x00070001..=0x00070003 => None, //Keyboard error codes.
    0x00070004 => Some(EfiKey::EfiKeyC1),
    0x00070005 => Some(EfiKey::EfiKeyB5),
    0x00070006 => Some(EfiKey::EfiKeyB3),
    0x00070007 => Some(EfiKey::EfiKeyC3),
    0x00070008 => Some(EfiKey::EfiKeyD3),
    0x00070009 => Some(EfiKey::EfiKeyC4),
    0x0007000A => Some(EfiKey::EfiKeyC5),
    0x0007000B => Some(EfiKey::EfiKeyC6),
    0x0007000C => Some(EfiKey::EfiKeyD8),
    0x0007000D => Some(EfiKey::EfiKeyC7),
    0x0007000E => Some(EfiKey::EfiKeyC8),
    0x0007000F => Some(EfiKey::EfiKeyC9),
    0x00070010 => Some(EfiKey::EfiKeyB7),
    0x00070011 => Some(EfiKey::EfiKeyB6),
    0x00070012 => Some(EfiKey::EfiKeyD9),
    0x00070013 => Some(EfiKey::EfiKeyD10),
    0x00070014 => Some(EfiKey::EfiKeyD1),
    0x00070015 => Some(EfiKey::EfiKeyD4),
    0x00070016 => Some(EfiKey::EfiKeyC2),
    0x00070017 => Some(EfiKey::EfiKeyD5),
    0x00070018 => Some(EfiKey::EfiKeyD7),
    0x00070019 => Some(EfiKey::EfiKeyB4),
    0x0007001A => Some(EfiKey::EfiKeyD2),
    0x0007001B => Some(EfiKey::EfiKeyB2),
    0x0007001C => Some(EfiKey::EfiKeyD6),
    0x0007001D => Some(EfiKey::EfiKeyB1),
    0x0007001E => Some(EfiKey::EfiKeyE1),
    0x0007001F => Some(EfiKey::EfiKeyE2),
    0x00070020 => Some(EfiKey::EfiKeyE3),
    0x00070021 => Some(EfiKey::EfiKeyE4),
    0x00070022 => Some(EfiKey::EfiKeyE5),
    0x00070023 => Some(EfiKey::EfiKeyE6),
    0x00070024 => Some(EfiKey::EfiKeyE7),
    0x00070025 => Some(EfiKey::EfiKeyE8),
    0x00070026 => Some(EfiKey::EfiKeyE9),
    0x00070027 => Some(EfiKey::EfiKeyE10),
    0x00070028 => Some(EfiKey::EfiKeyEnter),
    0x00070029 => Some(EfiKey::EfiKeyEsc),
    0x0007002A => Some(EfiKey::EfiKeyBackSpace),
    0x0007002B => Some(EfiKey::EfiKeyTab),
    0x0007002C => Some(EfiKey::EfiKeySpaceBar),
    0x0007002D => Some(EfiKey::EfiKeyE11),
    0x0007002E => Some(EfiKey::EfiKeyE12),
    0x0007002F => Some(EfiKey::EfiKeyD11),
    0x00070030 => Some(EfiKey::EfiKeyD12),
    0x00070031 => Some(EfiKey::EfiKeyD13),
    0x00070032 => Some(EfiKey::EfiKeyC12),
    0x00070033 => Some(EfiKey::EfiKeyC10),
    0x00070034 => Some(EfiKey::EfiKeyC11),
    0x00070035 => Some(EfiKey::EfiKeyE0),
    0x00070036 => Some(EfiKey::EfiKeyB8),
    0x00070037 => Some(EfiKey::EfiKeyB9),
    0x00070038 => Some(EfiKey::EfiKeyB10),
    0x00070039 => Some(EfiKey::EfiKeyCapsLock),
    0x0007003A => Some(EfiKey::EfiKeyF1),
    0x0007003B => Some(EfiKey::EfiKeyF2),
    0x0007003C => Some(EfiKey::EfiKeyF3),
    0x0007003D => Some(EfiKey::EfiKeyF4),
    0x0007003E => Some(EfiKey::EfiKeyF5),
    0x0007003F => Some(EfiKey::EfiKeyF6),
    0x00070040 => Some(EfiKey::EfiKeyF7),
    0x00070041 => Some(EfiKey::EfiKeyF8),
    0x00070042 => Some(EfiKey::EfiKeyF9),
    0x00070043 => Some(EfiKey::EfiKeyF10),
    0x00070044 => Some(EfiKey::EfiKeyF11),
    0x00070045 => Some(EfiKey::EfiKeyF12),
    0x00070046 => Some(EfiKey::EfiKeyPrint),
    0x00070047 => Some(EfiKey::EfiKeySLck),
    0x00070048 => Some(EfiKey::EfiKeyPause),
    0x00070049 => Some(EfiKey::EfiKeyIns),
    0x0007004A => Some(EfiKey::EfiKeyHome),
    0x0007004B => Some(EfiKey::EfiKeyPgUp),
    0x0007004C => Some(EfiKey::EfiKeyDel),
    0x0007004D => Some(EfiKey::EfiKeyEnd),
    0x0007004E => Some(EfiKey::EfiKeyPgDn),
    0x0007004F => Some(EfiKey::EfiKeyRightArrow),
    0x00070050 => Some(EfiKey::EfiKeyLeftArrow),
    0x00070051 => Some(EfiKey::EfiKeyDownArrow),
    0x00070052 => Some(EfiKey::EfiKeyUpArrow),
    0x00070053 => Some(EfiKey::EfiKeyNLck),
    0x00070054 => Some(EfiKey::EfiKeySlash),
    0x00070055 => Some(EfiKey::EfiKeyAsterisk),
    0x00070056 => Some(EfiKey::EfiKeyMinus),
    0x00070057 => Some(EfiKey::EfiKeyPlus),
    0x00070058 => Some(EfiKey::EfiKeyEnter),
    0x00070059 => Some(EfiKey::EfiKeyOne),
    0x0007005A => Some(EfiKey::EfiKeyTwo),
    0x0007005B => Some(EfiKey::EfiKeyThree),
    0x0007005C => Some(EfiKey::EfiKeyFour),
    0x0007005D => Some(EfiKey::EfiKeyFive),
    0x0007005E => Some(EfiKey::EfiKeySix),
    0x0007005F => Some(EfiKey::EfiKeySeven),
    0x00070060 => Some(EfiKey::EfiKeyEight),
    0x00070061 => Some(EfiKey::EfiKeyNine),
    0x00070062 => Some(EfiKey::EfiKeyZero),
    0x00070063 => Some(EfiKey::EfiKeyPeriod),
    0x00070064 => Some(EfiKey::EfiKeyB0),
    0x00070065 => Some(EfiKey::EfiKeyA4),
    0x00070066..=0x000700DF => None, // not used by EFI keyboard layout.
    0x000700E0 => Some(EfiKey::EfiKeyLCtrl),
    0x000700E1 => Some(EfiKey::EfiKeyLShift),
    0x000700E2 => Some(EfiKey::EfiKeyLAlt),
    0x000700E3 => Some(EfiKey::EfiKeyA0),
    0x000700E4 => Some(EfiKey::EfiKeyRCtrl),
    0x000700E5 => Some(EfiKey::EfiKeyRShift),
    0x000700E6 => Some(EfiKey::EfiKeyA2),
    0x000700E7 => Some(EfiKey::EfiKeyA3),
    _ => None, // all other usages not used by EFI keyboard layout.
  }
}

//These should be defined in r_efi::protocols::simple_text_input
const SCAN_NULL: u16 = 0x0000;
const SCAN_UP: u16 = 0x0001;
const SCAN_DOWN: u16 = 0x0002;
const SCAN_RIGHT: u16 = 0x0003;
const SCAN_LEFT: u16 = 0x0004;
const SCAN_HOME: u16 = 0x0005;
const SCAN_END: u16 = 0x0006;
const SCAN_INSERT: u16 = 0x0007;
const SCAN_DELETE: u16 = 0x0008;
const SCAN_PAGE_UP: u16 = 0x0009;
const SCAN_PAGE_DOWN: u16 = 0x000A;
const SCAN_F1: u16 = 0x000B;
const SCAN_F2: u16 = 0x000C;
const SCAN_F3: u16 = 0x000D;
const SCAN_F4: u16 = 0x000E;
const SCAN_F5: u16 = 0x000F;
const SCAN_F6: u16 = 0x0010;
const SCAN_F7: u16 = 0x0011;
const SCAN_F8: u16 = 0x0012;
const SCAN_F9: u16 = 0x0013;
const SCAN_F10: u16 = 0x0014;
const SCAN_F11: u16 = 0x0015;
const SCAN_F12: u16 = 0x0016;
const SCAN_ESC: u16 = 0x0017;
const SCAN_PAUSE: u16 = 0x0048;

// helper routine that converts the given modifier to the corresponding SCAN code
fn modifier_to_scan(modifier: u16) -> u16 {
  match modifier {
    INSERT_MODIFIER => SCAN_INSERT,
    DELETE_MODIFIER => SCAN_DELETE,
    PAGE_DOWN_MODIFIER => SCAN_PAGE_DOWN,
    PAGE_UP_MODIFIER => SCAN_PAGE_UP,
    HOME_MODIFIER => SCAN_HOME,
    END_MODIFIER => SCAN_END,
    LEFT_ARROW_MODIFIER => SCAN_LEFT,
    RIGHT_ARROW_MODIFIER => SCAN_RIGHT,
    DOWN_ARROW_MODIFIER => SCAN_DOWN,
    UP_ARROW_MODIFIER => SCAN_UP,
    FUNCTION_KEY_ONE_MODIFIER => SCAN_F1,
    FUNCTION_KEY_TWO_MODIFIER => SCAN_F2,
    FUNCTION_KEY_THREE_MODIFIER => SCAN_F3,
    FUNCTION_KEY_FOUR_MODIFIER => SCAN_F4,
    FUNCTION_KEY_FIVE_MODIFIER => SCAN_F5,
    FUNCTION_KEY_SIX_MODIFIER => SCAN_F6,
    FUNCTION_KEY_SEVEN_MODIFIER => SCAN_F7,
    FUNCTION_KEY_EIGHT_MODIFIER => SCAN_F8,
    FUNCTION_KEY_NINE_MODIFIER => SCAN_F9,
    FUNCTION_KEY_TEN_MODIFIER => SCAN_F10,
    FUNCTION_KEY_ELEVEN_MODIFIER => SCAN_F11,
    FUNCTION_KEY_TWELVE_MODIFIER => SCAN_F12,
    PAUSE_MODIFIER => SCAN_PAUSE,
    _ => SCAN_NULL,
  }
}

// helper routine that converts the given modifer to the corresponding HID Usage.
fn modifer_to_led_usage(modifier: u16) -> Option<Usage> {
  match modifier {
    NUM_LOCK_MODIFIER => Some(Usage::from(0x00080001)),
    CAPS_LOCK_MODIFIER => Some(Usage::from(0x00080002)),
    SCROLL_LOCK_MODIFIER => Some(Usage::from(0x00080003)),
    _ => None,
  }
}
