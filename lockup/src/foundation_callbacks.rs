use crate::*;
use unc_sdk::{unc, PromiseOrValue, Gas, assert_self, is_promise_success};
use std::convert::Into;

#[unc]
impl LockupContract {
    /// Called after the request to get the current staked balance to unstake everything for vesting
    /// schedule termination.
    pub fn on_get_account_staked_balance_to_unstake(
        &mut self,
        #[callback] staked_balance: WrappedBalance,
    ) -> PromiseOrValue<bool> {
        assert_self();
        if staked_balance.0 > 0 {
            // Need to unstake
            env::log_str(
                format!(
                    "Termination Step: Unstaking {} from the staking pool @{}",
                    staked_balance.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );

            ext_staking_pool::ext(self
                    .staking_information
                    .as_ref()
                    .unwrap()
                    .staking_pool_account_id
                    .clone())
                .with_static_gas(Gas::from_gas(gas::staking_pool::UNSTAKE))
                .with_attached_deposit(NO_DEPOSIT)
                .unstake(
                    staked_balance,
            )
            .then(
                ext_self_foundation::ext(env::current_account_id())
                    .with_static_gas(Gas::from_gas(gas::foundation_callbacks::ON_STAKING_POOL_UNSTAKE_FOR_TERMINATION))
                    .with_attached_deposit(NO_DEPOSIT)
                    .on_staking_pool_unstake_for_termination(
                        staked_balance,
                ),
            )
            .into()
        } else {
            env::log_str("Termination Step: Nothing to unstake. Moving to the next status.");
            self.set_staking_pool_status(TransactionStatus::Idle);
            self.set_termination_status(TerminationStatus::EverythingUnstaked);
            PromiseOrValue::Value(true)
        }
    }

    /// Called after the given amount is unstaked from the staking pool contract due to vesting
    /// termination.
    pub fn on_staking_pool_unstake_for_termination(&mut self, amount: WrappedBalance) -> bool {
        assert_self();

        let unstake_succeeded = is_promise_success();
        self.set_staking_pool_status(TransactionStatus::Idle);

        if unstake_succeeded {
            self.set_termination_status(TerminationStatus::EverythingUnstaked);
            env::log_str(
                format!(
                    "Termination Step: Unstaking of {} at @{} succeeded",
                    amount.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );
        } else {
            self.set_termination_status(TerminationStatus::VestingTerminatedWithDeficit);
            env::log_str(
                format!(
                    "Termination Step: Unstaking {} at @{} has failed",
                    amount.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );
        }
        unstake_succeeded
    }

    /// Called after the request to get the current unstaked balance to withdraw everything for
    /// vesting schedule termination.
    pub fn on_get_account_unstaked_balance_to_withdraw(
        &mut self,
        #[callback] unstaked_balance: WrappedBalance,
    ) -> PromiseOrValue<bool> {
        assert_self();
        if unstaked_balance.0 > 0 {
            // Need to withdraw
            env::log_str(
                format!(
                    "Termination Step: Withdrawing {} from the staking pool @{}",
                    unstaked_balance.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );

            ext_staking_pool::ext(self
                    .staking_information
                    .as_ref()
                    .unwrap()
                    .staking_pool_account_id
                    .clone())
                .with_static_gas(Gas::from_gas(gas::staking_pool::WITHDRAW))
                .with_attached_deposit(NO_DEPOSIT)
                .withdraw(
                    unstaked_balance,
            )
            .then(
                ext_self_foundation::ext(env::current_account_id())
                    .with_static_gas(Gas::from_gas(gas::foundation_callbacks::ON_STAKING_POOL_WITHDRAW_FOR_TERMINATION))
                    .with_attached_deposit(NO_DEPOSIT)
                    .on_staking_pool_withdraw_for_termination(
                        unstaked_balance,
                ),
            )
            .into()
        } else {
            env::log_str("Termination Step: Nothing to withdraw from the staking pool. Ready to withdraw from the account.");
            self.set_staking_pool_status(TransactionStatus::Idle);
            self.set_termination_status(TerminationStatus::ReadyToWithdraw);
            PromiseOrValue::Value(true)
        }
    }

    /// Called after the given amount is unstaked from the staking pool contract due to vesting
    /// termination.
    pub fn on_staking_pool_withdraw_for_termination(&mut self, amount: WrappedBalance) -> bool {
        assert_self();

        let withdraw_succeeded = is_promise_success();
        self.set_staking_pool_status(TransactionStatus::Idle);

        if withdraw_succeeded {
            self.set_termination_status(TerminationStatus::ReadyToWithdraw);
            {
                let staking_information = self.staking_information.as_mut().unwrap();
                // Due to staking rewards the deposit amount can become negative.
                staking_information.deposit_amount.0 = staking_information
                    .deposit_amount
                    .0
                    .saturating_sub(amount.0);
            }
            env::log_str(
                format!(
                    "Termination Step: The withdrawal of {} from @{} succeeded",
                    amount.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );
        } else {
            self.set_termination_status(TerminationStatus::EverythingUnstaked);
            env::log_str(
                format!(
                    "Termination Step: The withdrawal of {} from @{} failed",
                    amount.0,
                    self.staking_information
                        .as_ref()
                        .unwrap()
                        .staking_pool_account_id
                )
                .as_str(),
            );
        }
        withdraw_succeeded
    }

    /// Called after the foundation tried to withdraw the terminated unvested balance.
    pub fn on_withdraw_unvested_amount(
        &mut self,
        amount: WrappedBalance,
        receiver_id: AccountId,
    ) -> bool {
        assert_self();

        let withdraw_succeeded = is_promise_success();
        if withdraw_succeeded {
            env::log_str(
                format!(
                    "Termination Step: The withdrawal of the terminated unvested amount of {} to @{} succeeded.",
                    amount.0, receiver_id
                )
                    .as_str(),
            );
            // Decreasing lockup amount after withdrawal.
            self.lockup_information.termination_withdrawn_tokens += amount.0;
            let unvested_amount = self.get_terminated_unvested_balance().0;
            if unvested_amount > amount.0 {
                // There is still unvested balance remaining.
                let remaining_balance = unvested_amount - amount.0;
                self.vesting_information =
                    VestingInformation::Terminating(TerminationInformation {
                        unvested_amount: remaining_balance.into(),
                        status: TerminationStatus::ReadyToWithdraw,
                    });
                env::log_str(
                    format!(
                        "Termination Step: There is still terminated unvested balance of {} remaining to be withdrawn",
                        remaining_balance
                    )
                        .as_str(),
                );
                if self.get_account_balance().0 == 0 {
                    env::log_str("The withdrawal is completed: no more balance can be withdrawn in a future call");
                }
            } else {
                self.foundation_account_id = None;
                self.vesting_information = VestingInformation::None;
                env::log_str("Vesting schedule termination and withdrawal are completed");
            }
        } else {
            self.set_termination_status(TerminationStatus::ReadyToWithdraw);
            env::log_str(
                format!(
                    "Termination Step: The withdrawal of the terminated unvested amount of {} to @{} failed",
                    amount.0, receiver_id,
                )
                .as_str(),
            );
        }
        withdraw_succeeded
    }
}
