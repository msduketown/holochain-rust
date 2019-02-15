extern crate futures;
extern crate serde_json;
use crate::{
    action::{Action, ActionWrapper},
    context::Context,
    nucleus::ribosome::callback::{self, CallbackResult},
};
use futures::{future::Future, task::{LocalWaker, Poll}};
use holochain_core_types::{
    cas::content::AddressableContent,
    entry::{entry_type::EntryType, Entry},
    error::HolochainError,
    hash::HashString,
    validation::ValidationData,
};
use snowflake::{self, ProcessUniqueId};
use std::{pin::Pin, sync::Arc, thread};

fn check_entry_type(entry_type: EntryType, context: &Arc<Context>) -> Result<(), HolochainError> {
    match entry_type {
        EntryType::App(app_entry_type) => {
            // Check if app_entry_type is defined in DNA
            let _ = context
                .state()
                .unwrap()
                .nucleus()
                .dna()
                .unwrap()
                .get_zome_name_for_app_entry_type(&app_entry_type)
                .ok_or(HolochainError::ValidationFailed(
                    format!(
                        "Attempted to validate unknown app entry type {:?}",
                        app_entry_type,
                    ),
                ))?;
        }

        EntryType::LinkAdd => {}
        EntryType::LinkRemove => {}
        EntryType::Deletion => {}
        EntryType::CapTokenGrant => {}
        EntryType::AgentId => {}

        _ => {
            return Err(HolochainError::ValidationFailed(
                format!(
                    "Attempted to validate system entry type {:?}",
                    entry_type,
                ),
            ));
        }
    }

    Ok(())
}

fn spawn_validation_ribosome(
    id: ProcessUniqueId,
    entry: Entry,
    validation_data: ValidationData,
    context: Arc<Context>
) {
    thread::spawn(move || {
        let address = entry.address();
        let maybe_validation_result = callback::validate_entry::validate_entry(
            entry.clone(),
            validation_data.clone(),
            context.clone(),
        );

        let result = match maybe_validation_result {
            Ok(validation_result) => match validation_result {
                CallbackResult::Fail(error_string) => Err(error_string),
                CallbackResult::Pass => Ok(()),
                CallbackResult::NotImplemented(reason) => Err(format!(
                    "Validation callback not implemented for {:?} ({})",
                    entry.entry_type().clone(),
                    reason
                )),
                _ => unreachable!(),
            },
            Err(error) => Err(error.to_string()),
        };

        context
            .action_channel()
            .send(ActionWrapper::new(Action::ReturnValidationResult((
                (id, address),
                result,
            ))))
            .expect("action channel to be open in reducer");
    });
}

/// ValidateEntry Action Creator
/// This is the high-level validate function that wraps the whole validation process and is what should
/// be called from zome api functions and other contexts that don't care about implementation details.
///
/// Returns a future that resolves to an Ok(ActionWrapper) or an Err(error_message:String).
pub async fn validate_entry(
    entry: Entry,
    validation_data: ValidationData,
    context: &Arc<Context>,
) -> Result<HashString, HolochainError> {
    let id = snowflake::ProcessUniqueId::new();
    let address = entry.address();

    check_entry_type(entry.entry_type(), context)?;
    spawn_validation_ribosome(id.clone(), entry, validation_data, context.clone());

    await!(ValidationFuture {
        context: context.clone(),
        key: (id, address),
    })
}

/// ValidationFuture resolves to an Ok(ActionWrapper) or an Err(error_message:String).
/// Tracks the state for ValidationResults.
pub struct ValidationFuture {
    context: Arc<Context>,
    key: (snowflake::ProcessUniqueId, HashString),
}

impl Future for ValidationFuture {
    type Output = Result<HashString, HolochainError>;

    fn poll(self: Pin<&mut Self>, lw: &LocalWaker) -> Poll<Self::Output> {
        //
        // TODO: connect the waker to state updates for performance reasons
        // See: https://github.com/holochain/holochain-rust/issues/314
        //
        lw.wake();
        if let Some(state) = self.context.state() {
            match state.nucleus().validation_results.get(&self.key) {
                Some(Ok(())) => Poll::Ready(Ok(self.key.1.clone())),
                Some(Err(e)) => Poll::Ready(Err(HolochainError::ValidationFailed(e.clone()))),
                None => Poll::Pending,
            }
        } else {
            Poll::Pending
        }
    }
}
