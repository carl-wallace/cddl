#![cfg(feature = "std")]

use crate::{
  ast::*,
  token::{self, Token},
  visitor::{self, *},
};
use serde_cbor::Value;
use std::fmt;

use super::*;

/// cbor validation Result
pub type Result = std::result::Result<(), Error>;

/// cbor validation error
#[derive(Debug)]
pub enum Error {
  /// Zero or more validation errors
  Validation(Vec<ValidationError>),
  /// cbor parsing error
  CBORParsing(serde_cbor::Error),
  /// CDDL parsing error
  CDDLParsing(String),
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Error::Validation(errors) => {
        let mut error_str = String::new();
        for e in errors.iter() {
          error_str.push_str(&format!("{}\n", e));
        }
        write!(f, "{}", error_str)
      }
      Error::CBORParsing(error) => write!(f, "error parsing cbor: {}", error),
      Error::CDDLParsing(error) => write!(f, "error parsing CDDL: {}", error),
    }
  }
}

impl std::error::Error for Error {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      Error::CBORParsing(error) => Some(error),
      _ => None,
    }
  }
}

/// cbor validation error
#[derive(Clone, Debug)]
pub struct ValidationError {
  /// Error message
  pub reason: String,
  /// Location in CDDL where error occurred
  pub cddl_location: String,
  /// Location in CBOR where error occurred
  pub cbor_location: String,
  /// Whether or not the error is associated with multiple type choices
  pub is_multi_type_choice: bool,
  /// Whether or not the error is associated with multiple group choices
  pub is_multi_group_choice: bool,
  /// Whether or not the error is associated with a group to choice enumeration
  pub is_group_to_choice_enum: bool,
  /// Error is associated with a type/group name group entry
  pub type_group_name_entry: Option<String>,
}

impl fmt::Display for ValidationError {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    let mut error_str = String::from("error validating");
    if self.is_multi_group_choice {
      error_str.push_str(" group choice");
    }
    if self.is_multi_type_choice {
      error_str.push_str(" type choice");
    }
    if self.is_group_to_choice_enum {
      error_str.push_str(" type choice in group to choice enumeration");
    }
    if let Some(entry) = &self.type_group_name_entry {
      error_str.push_str(&format!(" group entry associated with rule \"{}\"", entry));
    }

    write!(
      f,
      "{} at cddl location \"{}\" and cbor location {}: {}",
      error_str, self.cddl_location, self.cbor_location, self.reason
    )
  }
}

impl std::error::Error for ValidationError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    None
  }
}

impl ValidationError {
  fn from_validator(jv: &CBORValidator, reason: String) -> Self {
    ValidationError {
      cddl_location: jv.cddl_location.clone(),
      cbor_location: jv.cbor_location.clone(),
      reason,
      is_multi_type_choice: jv.is_multi_type_choice,
      is_group_to_choice_enum: jv.is_group_to_choice_enum,
      type_group_name_entry: jv.type_group_name_entry.map(|e| e.to_string()),
      is_multi_group_choice: jv.is_multi_group_choice,
    }
  }
}

/// cbor validator type
pub struct CBORValidator<'a> {
  cddl: &'a CDDL<'a>,
  cbor: Value,
  errors: Vec<ValidationError>,
  cddl_location: String,
  cbor_location: String,
  // Occurrence indicator detected in current state of AST evaluation
  occurence: Option<Occur>,
  // Current group entry index detected in current state of AST evaluation
  group_entry_idx: Option<usize>,
  // cbor object value hoisted from previous state of AST evaluation
  object_value: Option<Value>,
  // Is member key detected in current state of AST evaluation
  is_member_key: bool,
  // Is a cut detected in current state of AST evaluation
  is_cut_present: bool,
  // Str value of cut detected in current state of AST evaluation
  cut_value: Option<Type1<'a>>,
  // Validate the generic rule given by str ident in current state of AST
  // evaluation
  eval_generic_rule: Option<&'a str>,
  // Aggregation of generic rules
  generic_rules: Vec<GenericRule<'a>>,
  // Control operator token detected in current state of AST evaluation
  ctrl: Option<token::Token<'a>>,
  // Is a group to choice enumeration detected in current state of AST
  // evaluation
  is_group_to_choice_enum: bool,
  // Are 2 or more type choices detected in current state of AST evaluation
  is_multi_type_choice: bool,
  // Are 2 or more group choices detected in current state of AST evaluation
  is_multi_group_choice: bool,
  // Type/group name entry detected in current state of AST evaluation. Used
  // only for providing more verbose error messages
  type_group_name_entry: Option<&'a str>,
  // Whether or not to advance to the next group entry if member key validation
  // fails as detected during the current state of AST evaluation
  advance_to_next_entry: bool,
  is_ctrl_map_equality: bool,
  entry_counts: Option<Vec<u64>>,
}

#[derive(Clone, Debug)]
struct GenericRule<'a> {
  name: &'a str,
  params: Vec<&'a str>,
  args: Vec<Type1<'a>>,
}

impl<'a> CBORValidator<'a> {
  /// New cborValidation from CDDL AST and cbor value
  pub fn new(cddl: &'a CDDL<'a>, cbor: Value) -> Self {
    CBORValidator {
      cddl,
      cbor,
      errors: Vec::default(),
      cddl_location: String::new(),
      cbor_location: String::new(),
      occurence: None,
      group_entry_idx: None,
      object_value: None,
      is_member_key: false,
      is_cut_present: false,
      cut_value: None,
      eval_generic_rule: None,
      generic_rules: Vec::new(),
      ctrl: None,
      is_group_to_choice_enum: false,
      is_multi_type_choice: false,
      is_multi_group_choice: false,
      type_group_name_entry: None,
      advance_to_next_entry: false,
      is_ctrl_map_equality: false,
      entry_counts: None,
    }
  }

  /// Validate
  pub fn validate(&mut self) -> std::result::Result<(), Error> {
    for r in self.cddl.rules.iter() {
      // First type rule is root
      if let Rule::Type { rule, .. } = r {
        if rule.generic_params.is_none() {
          self
            .visit_type_rule(rule)
            .map_err(|e| Error::Validation(vec![e]))?;
          break;
        }
      }
    }

    if !self.errors.is_empty() {
      return Err(Error::Validation(self.errors.clone()));
    }

    Ok(())
  }

  fn add_error(&mut self, reason: String) {
    self.errors.push(ValidationError {
      reason,
      cddl_location: self.cddl_location.clone(),
      cbor_location: self.cbor_location.clone(),
      is_multi_type_choice: self.is_multi_type_choice,
      is_multi_group_choice: self.is_multi_group_choice,
      is_group_to_choice_enum: self.is_group_to_choice_enum,
      type_group_name_entry: self.type_group_name_entry.map(|e| e.to_string()),
    });
  }
}

impl<'a> Visitor<'a, ValidationError> for CBORValidator<'a> {
  fn visit_type_rule(&mut self, tr: &TypeRule<'a>) -> visitor::Result<ValidationError> {
    if let Some(gp) = &tr.generic_params {
      if let Some(gr) = self
        .generic_rules
        .iter_mut()
        .find(|r| r.name == tr.name.ident)
      {
        gr.params = gp.params.iter().map(|p| p.param.ident).collect();
      } else {
        self.generic_rules.push(GenericRule {
          name: tr.name.ident,
          params: gp.params.iter().map(|p| p.param.ident).collect(),
          args: vec![],
        });
      }
    }

    let error_count = self.errors.len();

    for t in type_choice_alternates_from_ident(self.cddl, &tr.name) {
      let cur_errors = self.errors.len();
      self.visit_type(t)?;
      if self.errors.len() == cur_errors {
        for _ in 0..self.errors.len() - error_count {
          self.errors.pop();
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group_rule(&mut self, gr: &GroupRule<'a>) -> visitor::Result<ValidationError> {
    if let Some(gp) = &gr.generic_params {
      if let Some(gr) = self
        .generic_rules
        .iter_mut()
        .find(|r| r.name == gr.name.ident)
      {
        gr.params = gp.params.iter().map(|p| p.param.ident).collect();
      } else {
        self.generic_rules.push(GenericRule {
          name: gr.name.ident,
          params: gp.params.iter().map(|p| p.param.ident).collect(),
          args: vec![],
        });
      }
    }

    let error_count = self.errors.len();

    for ge in group_choice_alternates_from_ident(self.cddl, &gr.name) {
      let cur_errors = self.errors.len();
      self.visit_group_entry(ge)?;
      if self.errors.len() == cur_errors {
        for _ in 0..self.errors.len() - error_count {
          self.errors.pop();
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_type(&mut self, t: &Type<'a>) -> visitor::Result<ValidationError> {
    if t.type_choices.len() > 1 {
      self.is_multi_type_choice = true;
    }

    let initial_error_count = self.errors.len();
    for type_choice in t.type_choices.iter() {
      let error_count = self.errors.len();
      self.visit_type_choice(type_choice)?;
      if self.errors.len() == error_count {
        // Disregard invalid type choice validation errors if one of the
        // choices validates successfully
        let type_choice_error_count = self.errors.len() - initial_error_count;
        if type_choice_error_count > 0 {
          for _ in 0..type_choice_error_count {
            self.errors.pop();
          }
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group(&mut self, g: &Group<'a>) -> visitor::Result<ValidationError> {
    if g.group_choices.len() > 1 {
      self.is_multi_group_choice = true;
    }

    // Map equality/inequality validation
    if self.is_ctrl_map_equality {
      if let Some(t) = &self.ctrl {
        if let Value::Map(m) = &self.cbor {
          let mut entry_counts = Vec::new();
          for gc in g.group_choices.iter() {
            let count = entry_counts_from_group_choice(self.cddl, gc);
            entry_counts.push(count);
          }
          let len = m.len();
          if let Token::EQ = t {
            if !entry_counts.iter().any(|c| m.len() == *c as usize) {
              self.add_error(format!(
                "map equality error. expected object to have one of {:?} number of key/value pairs, got {}",
                entry_counts, len
              ));
              return Ok(());
            }
          } else if let Token::NE = t {
            if !entry_counts.iter().any(|c| m.len() != *c as usize) {
              self.add_error(format!(
                "map inequality error. expected object to not have one of {:?} number of key/value pairs, got {}",
                entry_counts, len
              ));
              return Ok(());
            }
          }
        }
      }
    }

    self.is_ctrl_map_equality = false;

    let initial_error_count = self.errors.len();
    for group_choice in g.group_choices.iter() {
      let error_count = self.errors.len();
      self.visit_group_choice(group_choice)?;
      if self.errors.len() == error_count {
        // Disregard invalid group choice validation errors if one of the
        // choices validates successfully
        let group_choice_error_count = self.errors.len() - initial_error_count;
        if group_choice_error_count > 0 {
          for _ in 0..group_choice_error_count {
            self.errors.pop();
          }
        }

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_group_choice(&mut self, gc: &GroupChoice<'a>) -> visitor::Result<ValidationError> {
    if self.is_group_to_choice_enum {
      let initial_error_count = self.errors.len();
      for tc in type_choices_from_group_choice(self.cddl, gc).iter() {
        let error_count = self.errors.len();
        self.visit_type_choice(tc)?;
        if self.errors.len() == error_count {
          let type_choice_error_count = self.errors.len() - initial_error_count;
          if type_choice_error_count > 0 {
            for _ in 0..type_choice_error_count {
              self.errors.pop();
            }
          }
          return Ok(());
        }
      }

      return Ok(());
    }

    for (idx, ge) in gc.group_entries.iter().enumerate() {
      self.group_entry_idx = Some(idx);

      self.visit_group_entry(&ge.0)?;
    }

    Ok(())
  }

  fn visit_range(
    &mut self,
    lower: &Type2,
    upper: &Type2,
    is_inclusive: bool,
  ) -> visitor::Result<ValidationError> {
    if let Value::Array(a) = &self.cbor {
      let allow_empty_array = matches!(self.occurence.as_ref(), Some(Occur::Optional(_)));

      #[allow(unused_assignments)]
      let mut iter_items = false;
      #[allow(unused_assignments)]
      match validate_array_occurrence(self.occurence.as_ref().take(), a) {
        Ok(r) => {
          iter_items = r;
        }
        Err(e) => {
          self.add_error(e);
          return Ok(());
        }
      }

      if !iter_items && !allow_empty_array {
        if let Some(entry_counts) = self.entry_counts.take() {
          let len = a.len();
          if !entry_counts.iter().any(|c| *c as usize == len) {
            self.add_error(format!(
              "expecting array with one of the lengths in {:?}, got {}",
              entry_counts, len
            ));
            return Ok(());
          }
        }
      }

      if iter_items {
        for (idx, v) in a.iter().enumerate() {
          let mut jv = CBORValidator::new(self.cddl, v.clone());
          jv.generic_rules = self.generic_rules.clone();
          jv.eval_generic_rule = self.eval_generic_rule;
          jv.is_multi_type_choice = self.is_multi_type_choice;
          jv.cbor_location
            .push_str(&format!("{}/{}", self.cbor_location, idx));

          jv.visit_range(lower, upper, is_inclusive)?;

          // If an array item is invalid, but a '?' or '*' occurrence indicator
          // is present, the ambiguity results in the error being disregarded
          // if !allow_errors {
          //   self.errors.append(&mut jv.errors);
          // }

          self.errors.append(&mut jv.errors);
        }
      } else if let Some(idx) = self.group_entry_idx.take() {
        if let Some(v) = a.get(idx) {
          let mut jv = CBORValidator::new(self.cddl, v.clone());
          jv.generic_rules = self.generic_rules.clone();
          jv.eval_generic_rule = self.eval_generic_rule;
          jv.is_multi_type_choice = self.is_multi_type_choice;
          jv.cbor_location
            .push_str(&format!("{}/{}", self.cbor_location, idx));

          jv.visit_range(lower, upper, is_inclusive)?;

          // If an array item is invalid, but a '?' or '*' occurrence indicator
          // is present, the ambiguity results in the error being disregarded
          // if !allow_errors {
          //   self.errors.append(&mut jv.errors);
          // }

          self.errors.append(&mut jv.errors);
        } else if !allow_empty_array {
          self.add_error(format!("expected array item at index {}", idx));
        }
      } else {
        self.add_error(format!(
          "expected range lower {} upper {} inclusive {}, got {:?}",
          lower, upper, is_inclusive, self.cbor
        ));
      }

      return Ok(());
    }

    match lower {
      Type2::IntValue { value: l, .. } => match upper {
        Type2::IntValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected integer to be in range {} <= value <= {}, got {:?}",
              l, u, self.cbor
            )
          } else {
            format!(
              "expected integer to be in range {} < value < {}, got {:?}",
              l, u, self.cbor
            )
          };

          match &self.cbor {
            Value::Integer(i) => {
              if is_inclusive {
                if *i < *l as i128 || *i > *u as i128 {
                  self.add_error(error_str);
                } else {
                  return Ok(());
                }
              } else if *i <= *l as i128 || *i >= *u as i128 {
                self.add_error(error_str);
                return Ok(());
              } else {
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        Type2::UintValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected integer to be in range {} <= value <= {}, got {:?}",
              l, u, self.cbor
            )
          } else {
            format!(
              "expected integer to be in range {} < value < {}, got {:?}",
              l, u, self.cbor
            )
          };

          match &self.cbor {
            Value::Integer(i) => {
              if is_inclusive {
                if *i < *l as i128 || *i > *u as i128 {
                  self.add_error(error_str);
                } else {
                  return Ok(());
                }
              } else if *i <= *l as i128 || *i >= *u as i128 {
                self.add_error(error_str);
                return Ok(());
              } else {
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be an integer type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      Type2::UintValue { value: l, .. } => match upper {
        Type2::UintValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected uint to be in range {} <= value <= {}, got {:?}",
              l, u, self.cbor
            )
          } else {
            format!(
              "expected uint to be in range {} < value < {}, got {:?}",
              l, u, self.cbor
            )
          };

          match &self.cbor {
            Value::Integer(i) => {
              if is_inclusive {
                if *i < *l as i128 || *i > *u as i128 {
                  self.add_error(error_str);
                } else {
                  return Ok(());
                }
              } else if *i <= *l as i128 || *i >= *u as i128 {
                self.add_error(error_str);
                return Ok(());
              } else {
                return Ok(());
              }
            }
            Value::Text(s) => match self.ctrl {
              Some(Token::SIZE) => {
                let len = s.len();
                let s = s.clone();
                if is_inclusive {
                  if s.len() < *l || s.len() > *u {
                    self.add_error(format!(
                      "expected \"{}\" string length to be in the range {} <= value <= {}, got {}",
                      s, l, u, len
                    ));
                    return Ok(());
                  } else {
                    return Ok(());
                  }
                } else if s.len() <= *l || s.len() >= *u {
                  self.add_error(format!(
                    "expected \"{}\" string length to be in the range {} < value < {}, got {}",
                    s, l, u, len
                  ));
                  return Ok(());
                }
              }
              _ => {
                self.add_error("string value cannot be validated against a range without the .size control operator".to_string());
                return Ok(());
              }
            },
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be a uint type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      Type2::FloatValue { value: l, .. } => match upper {
        Type2::FloatValue { value: u, .. } => {
          let error_str = if is_inclusive {
            format!(
              "expected float to be in range {} <= value <= {}, got {:?}",
              l, u, self.cbor
            )
          } else {
            format!(
              "expected float to be in range {} < value < {}, got {:?}",
              l, u, self.cbor
            )
          };

          match &self.cbor {
            Value::Float(f) => {
              if is_inclusive {
                if *f < *l as f64 || *f > *u as f64 {
                  self.add_error(error_str);
                } else {
                  return Ok(());
                }
              } else if *f <= *l as f64 || *f >= *u as f64 {
                self.add_error(error_str);
                return Ok(());
              } else {
                return Ok(());
              }
            }
            _ => {
              self.add_error(error_str);
              return Ok(());
            }
          }
        }
        _ => {
          self.add_error(format!(
            "invalid cddl range. upper value must be a float type. got {}",
            upper
          ));
          return Ok(());
        }
      },
      _ => {
        self.add_error(
          "invalid cddl range. upper and lower values must be either integers or floats"
            .to_string(),
        );

        return Ok(());
      }
    }

    Ok(())
  }

  fn visit_control_operator(
    &mut self,
    target: &Type2<'a>,
    ctrl: &str,
    controller: &Type2<'a>,
  ) -> visitor::Result<ValidationError> {
    match token::lookup_control_from_str(ctrl) {
      t @ Some(Token::EQ) => {
        match target {
          Type2::Typename { ident, .. } => {
            if is_ident_string_data_type(self.cddl, ident)
              || is_ident_numeric_data_type(self.cddl, ident)
            {
              return self.visit_type2(controller);
            }
          }
          Type2::Array { group, .. } => {
            if let Value::Array(_) = &self.cbor {
              let mut entry_counts = Vec::new();
              for gc in group.group_choices.iter() {
                let count = entry_counts_from_group_choice(self.cddl, gc);
                entry_counts.push(count);
              }
              self.entry_counts = Some(entry_counts);
              self.visit_type2(controller)?;
              self.entry_counts = None;
              return Ok(());
            }
          }
          Type2::Map { .. } => {
            if let Value::Map(_) = &self.cbor {
              self.ctrl = t;
              self.is_ctrl_map_equality = true;
              self.visit_type2(controller)?;
              self.ctrl = None;
              self.is_ctrl_map_equality = false;
              return Ok(());
            }
          }
          _ => self.add_error(format!(
            "target for .eq operator must be a string, numerical, array or map data type, got {}",
            target
          )),
        }
        Ok(())
      }
      t @ Some(Token::NE) => {
        match target {
          Type2::Typename { ident, .. } => {
            if is_ident_string_data_type(self.cddl, ident)
              || is_ident_numeric_data_type(self.cddl, ident)
            {
              self.ctrl = t;
              self.visit_type2(controller)?;
              self.ctrl = None;
              return Ok(());
            }
          }
          Type2::Array { .. } => {
            if let Value::Array(_) = &self.cbor {
              self.ctrl = t;
              self.visit_type2(controller)?;
              self.ctrl = None;
              return Ok(());
            }
          }
          Type2::Map { .. } => {
            if let Value::Map(_) = &self.cbor {
              self.ctrl = t;
              self.is_ctrl_map_equality = true;
              self.visit_type2(controller)?;
              self.ctrl = None;
              self.is_ctrl_map_equality = false;
              return Ok(());
            }
          }
          _ => self.add_error(format!(
            "target for .ne operator must be a string, numerical, array or map data type, got {}",
            target
          )),
        }
        Ok(())
      }
      t @ Some(Token::LT) | t @ Some(Token::GT) | t @ Some(Token::GE) | t @ Some(Token::LE) => {
        match target {
          Type2::Typename { ident, .. } if is_ident_numeric_data_type(self.cddl, ident) => {
            self.ctrl = t;
            self.visit_type2(controller)?;
            self.ctrl = None;
            Ok(())
          }
          _ => {
            self.add_error(format!(
              "target for .lt, .gt, .ge or .le operator must be a numerical data type, got {}",
              target
            ));
            Ok(())
          }
        }
      }
      t @ Some(Token::SIZE) => match target {
        Type2::Typename { ident, .. }
          if is_ident_string_data_type(self.cddl, ident)
            || is_ident_uint_data_type(self.cddl, ident) =>
        {
          self.ctrl = t;
          self.visit_type2(controller)?;
          self.ctrl = None;
          Ok(())
        }
        _ => {
          self.add_error(format!(
            "target for .size must a string or uint data type, got {}",
            target
          ));
          Ok(())
        }
      },
      t @ Some(Token::AND) => {
        self.ctrl = t;
        self.visit_type2(target)?;
        self.visit_type2(controller)?;
        self.ctrl = None;
        Ok(())
      }
      t @ Some(Token::WITHIN) => {
        self.ctrl = t;
        let error_count = self.errors.len();
        self.visit_type2(target)?;
        let no_errors = self.errors.len() == error_count;
        self.visit_type2(controller)?;
        if no_errors && self.errors.len() > error_count {
          for _ in 0..self.errors.len() - error_count {
            self.errors.pop();
          }

          self.add_error(format!(
            "expected type {} .within type {}, got {:?}",
            target, controller, self.cbor,
          ));
        }

        self.ctrl = None;

        Ok(())
      }
      t @ Some(Token::DEFAULT) => {
        self.ctrl = t;
        let error_count = self.errors.len();
        self.visit_type2(target)?;
        if self.errors.len() != error_count {
          if let Some(Occur::Optional(_)) = self.occurence.take() {
            self.add_error(format!(
              "expected default value {}, got {:?}",
              controller, self.cbor
            ));
          }
        }
        self.ctrl = None;
        Ok(())
      }
      t @ Some(Token::REGEXP) | t @ Some(Token::PCRE) => {
        self.ctrl = t;
        match target {
          Type2::Typename { ident, .. } if is_ident_string_data_type(self.cddl, ident) => {
            match self.cbor {
              Value::Text(_) => self.visit_type2(controller)?,
              _ => self.add_error(format!(
                ".regexp/.pcre control can only be matched against cbor string, got {:?}",
                self.cbor
              )),
            }
          }
          _ => self.add_error(format!(
            ".regexp/.pcre contro9l can only be matched against string data type, got {}",
            target
          )),
        }
        self.ctrl = None;

        Ok(())
      }
      _ => {
        self.add_error(format!("unsupported control operator {}", ctrl));
        Ok(())
      }
    }
  }

  fn visit_type2(&mut self, t2: &Type2<'a>) -> visitor::Result<ValidationError> {
    match t2 {
      Type2::TextValue { value, .. } => self.visit_value(&token::Value::TEXT(value)),
      Type2::Map { group, .. } => match &self.cbor {
        Value::Map(_) => {
          self.visit_group(group)?;
          self.is_cut_present = false;
          self.cut_value = None;
          Ok(())
        }
        Value::Array(a) => {
          // Member keys are annotation only in an array context
          if self.is_member_key {
            return Ok(());
          }

          let allow_empty_array = matches!(self.occurence.as_ref(), Some(Occur::Optional(_)));

          #[allow(unused_assignments)]
          let mut iter_items = false;
          match validate_array_occurrence(self.occurence.as_ref().take(), a) {
            Ok(r) => {
              iter_items = r;
            }
            Err(e) => {
              self.add_error(e);
              return Ok(());
            }
          }

          if !iter_items && !allow_empty_array {
            if let Some(entry_counts) = self.entry_counts.take() {
              let len = a.len();
              if !entry_counts.iter().any(|c| *c as usize == len) {
                self.add_error(format!(
                  "expecting array with one of the lengths in {:?}, got {}",
                  entry_counts, len
                ));
                return Ok(());
              }
            }
          }

          if iter_items {
            for (idx, v) in a.iter().enumerate() {
              let mut jv = CBORValidator::new(self.cddl, v.clone());
              jv.generic_rules = self.generic_rules.clone();
              jv.eval_generic_rule = self.eval_generic_rule;
              jv.is_multi_type_choice = self.is_multi_type_choice;
              jv.cbor_location
                .push_str(&format!("{}/{}", self.cbor_location, idx));

              jv.visit_group(group)?;

              // If an array item is invalid, but a '?' or '*' occurrence indicator
              // is present, the ambiguity results in the error being disregarded
              // if !allow_errors {
              //   self.errors.append(&mut jv.errors);
              // }

              self.errors.append(&mut jv.errors);
            }
          } else if let Some(idx) = self.group_entry_idx.take() {
            if let Some(v) = a.get(idx) {
              let mut jv = CBORValidator::new(self.cddl, v.clone());
              jv.generic_rules = self.generic_rules.clone();
              jv.eval_generic_rule = self.eval_generic_rule;
              jv.is_multi_type_choice = self.is_multi_type_choice;
              jv.cbor_location
                .push_str(&format!("{}/{}", self.cbor_location, idx));

              jv.visit_group(group)?;

              // If an array item is invalid, but a '?' or '*' occurrence indicator
              // is present, the ambiguity results in the error being disregarded
              // if !allow_errors {
              //   self.errors.append(&mut jv.errors);
              // }

              self.errors.append(&mut jv.errors);
            } else if !allow_empty_array {
              self.add_error(format!("expected map object {} at index {}", group, idx));
            }
          } else {
            self.add_error(format!(
              "expected map object {}, got {:?}",
              group, self.cbor
            ));
          }

          Ok(())
        }
        _ => {
          self.add_error(format!("expected map object {}, got {:?}", t2, self.cbor));
          Ok(())
        }
      },
      Type2::Array { group, .. } => match &self.cbor {
        Value::Array(_) => {
          let mut entry_counts = Vec::new();
          for gc in group.group_choices.iter() {
            let count = entry_counts_from_group_choice(self.cddl, gc);
            entry_counts.push(count);
          }
          self.entry_counts = Some(entry_counts);
          self.visit_group(group)?;
          self.entry_counts = None;
          Ok(())
        }
        _ => {
          self.add_error(format!("expected array type, got {:?}", self.cbor));
          Ok(())
        }
      },
      Type2::ChoiceFromGroup {
        ident,
        generic_args,
        ..
      } => {
        if let Some(ga) = generic_args {
          if let Some(rule) = rule_from_ident(self.cddl, ident) {
            if let Some(gr) = self
              .generic_rules
              .iter_mut()
              .find(|gr| gr.name == ident.ident)
            {
              for arg in ga.args.iter() {
                gr.args.push((*arg.arg).clone());
              }
            } else if let Some(params) = generic_params_from_rule(rule) {
              self.generic_rules.push(GenericRule {
                name: ident.ident,
                params,
                args: ga.args.iter().cloned().map(|arg| *arg.arg).collect(),
              });
            }

            let mut jv = CBORValidator::new(self.cddl, self.cbor.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = Some(ident.ident);
            jv.is_group_to_choice_enum = true;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.visit_rule(rule)?;

            self.errors.append(&mut jv.errors);

            return Ok(());
          }
        }

        if group_rule_from_ident(self.cddl, ident).is_none() {
          self.add_error(format!(
            "rule {} must be a group rule to turn it into a choice",
            ident
          ));
          return Ok(());
        }

        self.is_group_to_choice_enum = true;
        self.visit_identifier(ident)?;
        self.is_group_to_choice_enum = false;

        Ok(())
      }
      Type2::ChoiceFromInlineGroup { group, .. } => {
        self.is_group_to_choice_enum = true;
        self.visit_group(group)?;
        self.is_group_to_choice_enum = false;
        Ok(())
      }
      Type2::Typename {
        ident,
        generic_args,
        ..
      } => {
        if let Some(ga) = generic_args {
          if let Some(rule) = rule_from_ident(self.cddl, ident) {
            if let Some(gr) = self
              .generic_rules
              .iter_mut()
              .find(|gr| gr.name == ident.ident)
            {
              for arg in ga.args.iter() {
                gr.args.push((*arg.arg).clone());
              }
            } else if let Some(params) = generic_params_from_rule(rule) {
              self.generic_rules.push(GenericRule {
                name: ident.ident,
                params,
                args: ga.args.iter().cloned().map(|arg| *arg.arg).collect(),
              });
            }

            let mut jv = CBORValidator::new(self.cddl, self.cbor.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = Some(ident.ident);
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.visit_rule(rule)?;

            self.errors.append(&mut jv.errors);

            return Ok(());
          }
        }

        self.visit_identifier(ident)
      }
      Type2::IntValue { value, .. } => self.visit_value(&token::Value::INT(*value)),
      Type2::UintValue { value, .. } => self.visit_value(&token::Value::UINT(*value)),
      Type2::FloatValue { value, .. } => self.visit_value(&token::Value::FLOAT(*value)),
      Type2::ParenthesizedType { pt, .. } => self.visit_type(pt),
      Type2::Any(_) => Ok(()),
      _ => {
        self.add_error(format!(
          "unsupported data type for validating cbor, got {}",
          t2
        ));
        Ok(())
      }
    }
  }

  fn visit_identifier(&mut self, ident: &Identifier<'a>) -> visitor::Result<ValidationError> {
    if let Some(name) = self.eval_generic_rule {
      if let Some(gr) = self
        .generic_rules
        .iter()
        .cloned()
        .find(|gr| gr.name == name)
      {
        for (idx, gp) in gr.params.iter().enumerate() {
          if *gp == ident.ident {
            if let Some(arg) = gr.args.get(idx) {
              return self.visit_type1(arg);
            }
          }
        }
      }
    }

    if let Some(r) = rule_from_ident(self.cddl, ident) {
      return self.visit_rule(r);
    }

    match &self.cbor {
      Value::Null if is_ident_null_data_type(self.cddl, ident) => Ok(()),
      Value::Bytes(_) if is_ident_byte_string_data_type(self.cddl, ident) => Ok(()),
      Value::Bool(b) => match token::lookup_ident(ident.ident) {
        Token::BOOL => Ok(()),
        Token::TRUE if *b => Ok(()),
        Token::FALSE if !b => Ok(()),
        _ => {
          self.add_error(format!("expected type {}, got {:?}", ident, self.cbor));
          Ok(())
        }
      },
      Value::Integer(i) => match token::lookup_ident(ident.ident) {
        Token::INT | Token::INTEGER => Ok(()),
        Token::UINT if *i >= 0 => Ok(()),
        _ => {
          self.add_error(format!("expected type {}, got {:?}", ident, self.cbor));
          Ok(())
        }
      },
      Value::Float(_) => match token::lookup_ident(ident.ident) {
        Token::FLOAT
        | Token::FLOAT16
        | Token::FLOAT1632
        | Token::FLOAT32
        | Token::FLOAT3264
        | Token::FLOAT64 => Ok(()),
        _ => {
          self.add_error(format!("expected type {}, got {:?}", ident, self.cbor));
          Ok(())
        }
      },
      Value::Text(_) if is_ident_string_data_type(self.cddl, ident) => Ok(()),
      Value::Array(a) => {
        // Member keys are annotation only in an array context
        if self.is_member_key {
          return Ok(());
        }

        let allow_empty_array = matches!(self.occurence.as_ref(), Some(Occur::Optional(_)));

        #[allow(unused_assignments)]
        let mut iter_items = false;
        match validate_array_occurrence(self.occurence.as_ref().take(), a) {
          Ok(r) => {
            iter_items = r;
          }
          Err(e) => {
            self.add_error(e);
            return Ok(());
          }
        }

        if !iter_items && !allow_empty_array {
          if let Some(entry_counts) = self.entry_counts.take() {
            let len = a.len();
            if !entry_counts.iter().any(|c| *c as usize == len) {
              self.add_error(format!(
                "expecting array with one of the lengths in {:?}, got {}",
                entry_counts, len
              ));
              return Ok(());
            }
          }
        }

        if iter_items {
          for (idx, v) in a.iter().enumerate() {
            let mut jv = CBORValidator::new(self.cddl, v.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = self.eval_generic_rule;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.cbor_location
              .push_str(&format!("{}/{}", self.cbor_location, idx));

            jv.visit_identifier(ident)?;

            // If an array item is invalid, but a '?' or '*' occurrence indicator
            // is present, the ambiguity results in the error being disregarded
            // if !allow_errors {
            //   self.errors.append(&mut jv.errors);
            // }

            self.errors.append(&mut jv.errors);
          }
        } else if let Some(idx) = self.group_entry_idx.take() {
          if let Some(v) = a.get(idx) {
            let mut jv = CBORValidator::new(self.cddl, v.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = self.eval_generic_rule;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.cbor_location
              .push_str(&format!("{}/{}", self.cbor_location, idx));

            jv.visit_identifier(ident)?;

            // If an array item is invalid, but a '?' or '*' occurrence indicator
            // is present, the ambiguity results in the error being disregarded
            // if !allow_errors {
            //   self.errors.append(&mut jv.errors);
            // }

            self.errors.append(&mut jv.errors);
          } else if !allow_empty_array {
            self.add_error(format!("expected type {} at index {}", ident, idx));
          }
        } else {
          self.add_error(format!("expected type {}, got {:?}", ident, self.cbor));
        }

        Ok(())
      }
      Value::Map(m) => {
        if let Some(occur) = &self.occurence {
          if let Occur::ZeroOrMore(_) | Occur::OneOrMore(_) = occur {
            if let Occur::OneOrMore(_) = occur {
              if m.is_empty() {
                self.add_error(format!(
                  "map cannot be empty, require one oe more entries with key type {}",
                  ident
                ));
                return Ok(());
              }
            }

            if is_ident_string_data_type(self.cddl, ident) {
              if !m.keys().all(|k| matches!(k, Value::Text(_))) {
                self.add_error(format!("map requires entry keys of type {}", ident));
              }

              return Ok(());
            }

            if is_ident_integer_data_type(self.cddl, ident) {
              if !m.keys().all(|k| matches!(k, Value::Integer(_))) {
                self.add_error(format!("map requires entry keys of type {}", ident));
              }

              return Ok(());
            }
          }
        }

        self.visit_value(&token::Value::TEXT(ident.ident))
      }
      _ => {
        if let Some(cut_value) = self.cut_value.take() {
          self.add_error(format!(
            "cut present for member key {}. expected type {}, got {:?}",
            cut_value, ident, self.cbor
          ));
        } else {
          self.add_error(format!("expected type {}, got {:?}", ident, self.cbor));
        }
        Ok(())
      }
    }
  }

  fn visit_value_member_key_entry(
    &mut self,
    entry: &ValueMemberKeyEntry<'a>,
  ) -> visitor::Result<ValidationError> {
    if let Some(occur) = &entry.occur {
      self.visit_occurrence(occur)?;
    }

    let current_location = self.cbor_location.clone();

    if let Some(mk) = &entry.member_key {
      let error_count = self.errors.len();
      self.is_member_key = true;
      self.visit_memberkey(mk)?;
      self.is_member_key = false;

      // Move to next entry if member key validation fails
      if self.errors.len() != error_count {
        self.advance_to_next_entry = true;
        return Ok(());
      }
    }

    if let Some(v) = self.object_value.take() {
      let mut jv = CBORValidator::new(self.cddl, v);
      jv.generic_rules = self.generic_rules.clone();
      jv.eval_generic_rule = self.eval_generic_rule;
      jv.is_multi_type_choice = self.is_multi_type_choice;
      jv.is_multi_group_choice = self.is_multi_group_choice;
      jv.cbor_location.push_str(&self.cbor_location);
      jv.type_group_name_entry = self.type_group_name_entry;
      jv.visit_type(&entry.entry_type)?;

      self.cbor_location = current_location;

      // if !jv.errors.is_empty() {
      //   if let Some(occur) = &self.occurence {
      //     if let Occur::Optional(_) | Occur::ZeroOrMore(_) = occur {
      //       if !self.is_cut_present {
      //         return Ok(());
      //       }
      //     }
      //   }
      // }

      self.errors.append(&mut jv.errors);
      if entry.occur.is_some() {
        self.occurence = None;
      }

      Ok(())
    } else if !self.advance_to_next_entry {
      self.visit_type(&entry.entry_type)
    } else {
      Ok(())
    }
  }

  fn visit_type_groupname_entry(
    &mut self,
    entry: &TypeGroupnameEntry<'a>,
  ) -> visitor::Result<ValidationError> {
    self.type_group_name_entry = Some(entry.name.ident);
    walk_type_groupname_entry(self, entry)?;
    self.type_group_name_entry = None;

    Ok(())
  }

  fn visit_memberkey(&mut self, mk: &MemberKey<'a>) -> visitor::Result<ValidationError> {
    if let MemberKey::Type1 { is_cut, .. } = mk {
      self.is_cut_present = *is_cut;
    }

    walk_memberkey(self, mk)
  }

  fn visit_value(&mut self, value: &token::Value<'a>) -> visitor::Result<ValidationError> {
    let error: Option<String> = match &self.cbor {
      Value::Integer(i) => match value {
        token::Value::INT(v) => match &self.ctrl {
          Some(Token::NE) if *i != *v as i128 => None,
          Some(Token::LT) if *i < *v as i128 => None,
          Some(Token::LE) if *i <= *v as i128 => None,
          Some(Token::GT) if *i > *v as i128 => None,
          Some(Token::GE) if *i >= *v as i128 => None,
          None => {
            if *i == *v as i128 {
              None
            } else {
              Some(format!("expected value {}, got {}", v, i))
            }
          }
          _ => Some(format!(
            "expected value {} {}, got {}",
            self.ctrl.clone().unwrap(),
            v,
            i
          )),
        },
        token::Value::UINT(v) => match &self.ctrl {
          Some(Token::NE) if *i != *v as i128 => None,
          Some(Token::LT) if *i < *v as i128 => None,
          Some(Token::LE) if *i <= *v as i128 => None,
          Some(Token::GT) if *i > *v as i128 => None,
          Some(Token::GE) if *i >= *v as i128 => None,
          Some(Token::SIZE) if *i < 256i128.pow(*v as u32) => None,
          None => {
            if *i == *v as i128 {
              None
            } else {
              Some(format!("expected value {}, got {}", v, i))
            }
          }
          _ => Some(format!(
            "expected value {} {}, got {}",
            self.ctrl.clone().unwrap(),
            v,
            i
          )),
        },

        _ => Some(format!("expected {}, got {}", value, i)),
      },
      Value::Float(f) => match value {
        token::Value::FLOAT(v) => match &self.ctrl {
          Some(Token::NE) if (f - *v).abs() > std::f64::EPSILON => None,
          Some(Token::LT) if *f < *v as f64 => None,
          Some(Token::LE) if *f <= *v as f64 => None,
          Some(Token::GT) if *f > *v as f64 => None,
          Some(Token::GE) if *f >= *v as f64 => None,
          None => {
            if (f - *v).abs() < std::f64::EPSILON {
              None
            } else {
              Some(format!("expected value {}, got {}", v, f))
            }
          }
          _ => Some(format!(
            "expected value {} {}, got {}",
            self.ctrl.clone().unwrap(),
            v,
            f
          )),
        },
        _ => Some(format!("expected {}, got {}", value, f)),
      },
      Value::Text(s) => match value {
        token::Value::TEXT(t) => match &self.ctrl {
          Some(Token::NE) => {
            if s != t {
              None
            } else {
              Some(format!("expected {} .ne to \"{}\"", value, s))
            }
          }
          Some(Token::REGEXP) | Some(Token::PCRE) => {
            let re = regex::Regex::new(
              serde_json::from_str::<serde_json::Value>(&format!("\"{}\"", t))
                .map_err(|e| ValidationError::from_validator(self, e.to_string()))?
                .as_str()
                .ok_or_else(|| {
                  ValidationError::from_validator(self, "malformed regex".to_string())
                })?,
            )
            .map_err(|e| ValidationError::from_validator(self, e.to_string()))?;

            if re.is_match(s) {
              None
            } else {
              Some(format!("expected \"{}\" to match regex \"{}\"", s, t))
            }
          }
          _ => {
            if s == t {
              None
            } else if let Some(ctrl) = &self.ctrl {
              Some(format!("expected value {} {}, got \"{}\"", ctrl, value, s))
            } else {
              Some(format!("expected value {} got \"{}\"", value, s))
            }
          }
        },
        token::Value::UINT(u) => match &self.ctrl {
          Some(Token::SIZE) => {
            if s.len() == *u {
              None
            } else {
              Some(format!("expected \"{}\" .size {}, got {}", s, u, s.len()))
            }
          }
          _ => Some(format!("expected {}, got {}", u, s)),
        },
        token::Value::BYTE(token::ByteValue::UTF8(b)) if s.as_bytes() == b.as_ref() => None,
        token::Value::BYTE(token::ByteValue::B16(b)) if s.as_bytes() == b.as_ref() => None,
        token::Value::BYTE(token::ByteValue::B64(b)) if s.as_bytes() == b.as_ref() => None,
        _ => Some(format!("expected {}, got \"{}\"", value, s)),
      },
      Value::Array(a) => {
        // Member keys are annotation only in an array context
        if self.is_member_key {
          return Ok(());
        }

        let allow_empty_array = matches!(self.occurence.as_ref(), Some(Occur::Optional(_)));

        #[allow(unused_assignments)]
        let mut iter_items = false;
        match validate_array_occurrence(self.occurence.as_ref().take(), a) {
          Ok(r) => {
            iter_items = r;
          }
          Err(e) => {
            self.add_error(e);
            return Ok(());
          }
        }

        if !iter_items && !allow_empty_array {
          if let Some(entry_counts) = self.entry_counts.take() {
            let len = a.len();
            if !entry_counts.iter().any(|c| *c as usize == len) {
              self.add_error(format!(
                "expecting array with one of the lengths in {:?}, got {}",
                entry_counts, len
              ));
              return Ok(());
            }
          }
        }

        if iter_items {
          for (idx, v) in a.iter().enumerate() {
            let mut jv = CBORValidator::new(self.cddl, v.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = self.eval_generic_rule;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.cbor_location
              .push_str(&format!("{}/{}", self.cbor_location, idx));

            jv.visit_value(value)?;

            // If an array item is invalid, but a '?' or '*' occurrence indicator
            // is present, the ambiguity results in the error being disregarded
            // if !allow_errors {
            //   self.errors.append(&mut jv.errors);
            // }

            self.errors.append(&mut jv.errors);
          }
        } else if let Some(idx) = self.group_entry_idx.take() {
          if let Some(v) = a.get(idx) {
            let mut jv = CBORValidator::new(self.cddl, v.clone());
            jv.generic_rules = self.generic_rules.clone();
            jv.eval_generic_rule = self.eval_generic_rule;
            jv.is_multi_type_choice = self.is_multi_type_choice;
            jv.cbor_location
              .push_str(&format!("{}/{}", self.cbor_location, idx));

            jv.visit_value(value)?;

            // If an array item is invalid, but a '?' or '*' occurrence indicator
            // is present, the ambiguity results in the error being disregarded
            // if !allow_errors {
            //   self.errors.append(&mut jv.errors);
            // }

            self.errors.append(&mut jv.errors);
          } else if !allow_empty_array {
            self.add_error(format!("expected value {} at index {}", value, idx));
          }
        } else {
          self.add_error(format!("expected value {}, got {:?}", value, self.cbor));
        }

        None
      }
      Value::Map(o) => {
        if self.is_cut_present {
          self.cut_value = Some(Type1::from(value.clone()));
        }

        if let token::Value::TEXT("any") = value {
          return Ok(());
        }

        // Retrieve the value from key unless optional/zero or more, in which
        // case advance to next group entry
        if let Some(v) = o.get(&token_value_into_cbor_value(value.clone())) {
          self.object_value = Some(v.clone());
          self.cbor_location.push_str(&format!("/{}", value));

          None
        } else if let Some(Occur::Optional(_)) | Some(Occur::ZeroOrMore(_)) = &self.occurence.take()
        {
          self.advance_to_next_entry = true;
          None
        } else if let Some(Token::NE) = &self.ctrl {
          None
        } else {
          Some(format!("object missing key: \"{}\"", value))
        }
      }
      _ => Some(format!("expected {}, got {:?}", value, self.cbor)),
    };

    if let Some(e) = error {
      self.add_error(e);
    }

    Ok(())
  }

  fn visit_occurrence(&mut self, o: &Occurrence) -> visitor::Result<ValidationError> {
    self.occurence = Some(o.occur.clone());

    Ok(())
  }
}

/// Converts a CDDL value type to serde_cbor::Value
pub fn token_value_into_cbor_value(value: token::Value) -> serde_cbor::Value {
  match value {
    token::Value::UINT(i) => serde_cbor::Value::Integer(i as i128),
    token::Value::INT(i) => serde_cbor::Value::Integer(i as i128),
    token::Value::FLOAT(f) => serde_cbor::Value::Float(f),
    token::Value::TEXT(t) => serde_cbor::Value::Text(t.to_string()),
    token::Value::BYTE(b) => match b {
      ByteValue::UTF8(b) | ByteValue::B16(b) | ByteValue::B64(b) => {
        serde_cbor::Value::Bytes(b.into_owned())
      }
    },
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;

  use super::*;
  use crate::{cddl_from_str, lexer_from_str};

  #[test]
  fn validate() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let input = r#"thing = { * int => any}"#;

    let mut cbor = BTreeMap::new();
    cbor.insert(1, "test");

    let mut lexer = lexer_from_str(input);
    let cddl = cddl_from_str(&mut lexer, input, true)?;
    let cbor = serde_cbor::to_vec(&cbor).unwrap();

    let cbor_value = serde_cbor::from_slice::<Value>(&cbor).unwrap();

    let mut cv = CBORValidator::new(&cddl, cbor_value);
    cv.validate()?;

    Ok(())
  }
}
