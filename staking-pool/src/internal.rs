use crate::*;

impl StakingContract {
    /********************/
    /* Internal methods */
    /********************/

    /// Restakes the current `total_staked_balance` again.
    pub(crate) fn internal_restake(&mut self) {
        if self.paused {
            return;
        }
        // Stakes with the staking public key. If the public key is invalid the entire function
        // call will be rolled back.
        Promise::new(env::current_account_id())
            .stake(self.total_staked_balance, self.stake_public_key.clone())
            .then(ext_self::ext(env::current_account_id())
                .with_static_gas(ON_STAKE_ACTION_GAS)
                .with_attached_deposit(NO_DEPOSIT)
                .on_stake_action(),
            );
    }

    pub(crate) fn internal_deposit(&mut self) -> u128 {
        let account_id = env::predecessor_account_id();
        let mut account = self.internal_get_account(&account_id);
        let amount = env::attached_deposit();
        account.unstaked = account.unstaked.saturating_add(amount);
        self.internal_save_account(&account_id, &account);
        self.last_total_balance = self.last_total_balance.saturating_add(amount);

        env::log_str(
            format!(
                "@{} deposited {}. New unstaked balance is {}",
                account_id, amount, account.unstaked
            )
            .as_str(),
        );
        amount.as_attounc()
    }

    pub(crate) fn internal_withdraw(&mut self, amount: UncToken) {
        assert!(amount.as_attounc() > 0, "Withdrawal amount should be positive");

        let account_id = env::predecessor_account_id();
        let mut account = self.internal_get_account(&account_id);
        assert!(
            account.unstaked >= amount,
            "Not enough unstaked balance to withdraw"
        );
        assert!(
            account.unstaked_available_epoch_height <= env::epoch_height(),
            "The unstaked balance is not yet available due to unstaking delay"
        );
        account.unstaked = account.unstaked.saturating_sub(amount);
        self.internal_save_account(&account_id, &account);

        env::log_str(
            format!(
                "@{} withdrawing {}. New unstaked balance is {}",
                account_id, amount, account.unstaked
            )
            .as_str(),
        );

        Promise::new(account_id).transfer(amount);
        self.last_total_balance = self.last_total_balance.saturating_sub(amount);
    }

    pub(crate) fn internal_stake(&mut self, amount: UncToken) {
        assert!(amount.as_attounc() > 0, "Staking amount should be positive");

        let account_id = env::predecessor_account_id();
        let mut account = self.internal_get_account(&account_id);

        // Calculate the number of "stake" shares that the account will receive for staking the
        // given amount.
        let num_shares = self.num_shares_from_staked_amount_rounded_down(amount);
        assert!(
            num_shares.as_attounc() > 0,
            "The calculated number of \"stake\" shares received for staking should be positive"
        );
        // The amount of tokens the account will be charged from the unstaked balance.
        // Rounded down to avoid overcharging the account to guarantee that the account can always
        // unstake at least the same amount as staked.
        let charge_amount = self.staked_amount_from_num_shares_rounded_down(num_shares);
        assert!(
            charge_amount.as_attounc() > 0,
            "Invariant violation. Calculated staked amount must be positive, because \"stake\" share price should be at least 1"
        );

        assert!(
            account.unstaked >= charge_amount,
            "Not enough unstaked balance to stake"
        );
        account.unstaked = account.unstaked.saturating_sub(charge_amount);
        account.stake_shares = account.stake_shares.saturating_add(num_shares);
        self.internal_save_account(&account_id, &account);

        // The staked amount that will be added to the total to guarantee the "stake" share price
        // never decreases. The difference between `stake_amount` and `charge_amount` is paid
        // from the allocated STAKE_SHARE_PRICE_GUARANTEE_FUND.
        let stake_amount = self.staked_amount_from_num_shares_rounded_up(num_shares);

        self.total_staked_balance = self.total_staked_balance.saturating_add(stake_amount);
        self.total_stake_shares = self.total_stake_shares.saturating_add(num_shares);

        env::log_str(
            format!(
                "@{} staking {}. Received {} new staking shares. Total {} unstaked balance and {} staking shares",
                account_id, charge_amount, num_shares, account.unstaked, account.stake_shares
            )
                .as_str(),
        );
        env::log_str(
            format!(
                "Contract total staked balance is {}. Total number of shares {}",
                self.total_staked_balance, self.total_stake_shares
            )
            .as_str(),
        );
    }

    pub(crate) fn inner_unstake(&mut self, amount: u128) {
        assert!(amount > 0, "Unstaking amount should be positive");

        let account_id = env::predecessor_account_id();
        let mut account = self.internal_get_account(&account_id);

        assert!(
            self.total_staked_balance.as_attounc() > 0,
            "The contract doesn't have staked balance"
        );
        // Calculate the number of shares required to unstake the given amount.
        // NOTE: The number of shares the account will pay is rounded up.
        let num_shares = self.num_shares_from_staked_amount_rounded_up(UncToken::from_attounc(amount));
        assert!(
            num_shares.as_attounc() > 0,
            "Invariant violation. The calculated number of \"stake\" shares for unstaking should be positive"
        );
        assert!(
            account.stake_shares >= num_shares,
            "Not enough staked balance to unstake"
        );

        // Calculating the amount of tokens the account will receive by unstaking the corresponding
        // number of "stake" shares, rounding up.
        let receive_amount = self.staked_amount_from_num_shares_rounded_up(num_shares);
        assert!(
            receive_amount.as_attounc() > 0,
            "Invariant violation. Calculated staked amount must be positive, because \"stake\" share price should be at least 1"
        );

        account.stake_shares = account.stake_shares.saturating_add(num_shares);
        account.unstaked = account.unstaked.saturating_add(receive_amount);
        account.unstaked_available_epoch_height = env::epoch_height() + NUM_EPOCHS_TO_UNLOCK;
        self.internal_save_account(&account_id, &account);

        // The amount tokens that will be unstaked from the total to guarantee the "stake" share
        // price never decreases. The difference between `receive_amount` and `unstake_amount` is
        // paid from the allocated STAKE_SHARE_PRICE_GUARANTEE_FUND.
        let unstake_amount = self.staked_amount_from_num_shares_rounded_down(num_shares);

        self.total_staked_balance = self.total_staked_balance.saturating_sub(unstake_amount);
        self.total_stake_shares = self.total_stake_shares.saturating_sub(num_shares);

        env::log_str(
            format!(
                "@{} unstaking {}. Spent {} staking shares. Total {} unstaked balance and {} staking shares",
                account_id, receive_amount, num_shares, account.unstaked, account.stake_shares
            )
                .as_str(),
        );
        env::log_str(
            format!(
                "Contract total staked balance is {}. Total number of shares {}",
                self.total_staked_balance, self.total_stake_shares
            )
            .as_str(),
        );
    }

    /// Asserts that the method was called by the owner.
    pub(crate) fn assert_owner(&self) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "Can only be called by the owner"
        );
    }

    /// Distributes rewards after the new epoch. It's automatically called before every action.
    /// Returns true if the current epoch height is different from the last epoch height.
    pub(crate) fn internal_ping(&mut self) -> bool {
        let epoch_height = env::epoch_height();
        if self.last_epoch_height == epoch_height {
            return false;
        }
        self.last_epoch_height = epoch_height;

        // New total amount (both locked and unlocked balances).
        // NOTE: We need to subtract `attached_deposit` in case `ping` called from `deposit` call
        // since the attached deposit gets included in the `account_balance`, and we have not
        // accounted it yet.
        let total_balance =
            env::account_locked_balance().saturating_add(env::account_balance()).saturating_sub(env::attached_deposit());

        assert!(
            total_balance >= self.last_total_balance,
            "The new total balance should not be less than the old total balance"
        );
        let total_reward = total_balance.saturating_sub(self.last_total_balance);
        if total_reward.as_attounc() > 0 {
            // The validation fee that the contract owner takes.
            let owners_fee = self.reward_fee_fraction.multiply(total_reward);

            // Distributing the remaining reward to the delegators first.
            let remaining_reward = total_reward.saturating_sub(owners_fee);
            self.total_staked_balance = self.total_staked_balance.saturating_add(remaining_reward);

            // Now buying "stake" shares for the contract owner at the new share price.
            let num_shares = self.num_shares_from_staked_amount_rounded_down(owners_fee);
            if num_shares.as_attounc() > 0 {
                // Updating owner's inner account
                let owner_id = self.owner_id.clone();
                let mut account = self.internal_get_account(&owner_id);
                account.stake_shares = account.stake_shares.saturating_add(num_shares);
                self.internal_save_account(&owner_id, &account);
                // Increasing the total amount of "stake" shares.
                self.total_stake_shares = self.total_stake_shares.saturating_add(num_shares);
            }
            // Increasing the total staked balance by the owners fee, no matter whether the owner
            // received any shares or not.
            self.total_staked_balance = self.total_staked_balance.saturating_add(owners_fee);

            env::log_str(
                format!(
                    "Epoch {}: Contract received total rewards of {} tokens. New total staked balance is {}. Total number of shares {}",
                    epoch_height, total_reward, self.total_staked_balance, self.total_stake_shares,
                )
                    .as_str(),
            );
            if num_shares.as_attounc() > 0 {
                env::log_str(format!("Total rewards fee is {} stake shares.", num_shares).as_str());
            }
        }

        self.last_total_balance = total_balance;
        true
    }

    /// Returns the number of "stake" shares rounded down corresponding to the given staked balance
    /// amount.
    ///
    /// price = total_staked / total_shares
    /// Price is fixed
    /// (total_staked + amount) / (total_shares + num_shares) = total_staked / total_shares
    /// (total_staked + amount) * total_shares = total_staked * (total_shares + num_shares)
    /// amount * total_shares = total_staked * num_shares
    /// num_shares = amount * total_shares / total_staked
    pub(crate) fn num_shares_from_staked_amount_rounded_down(
        &self,
        amount: UncToken,
    ) -> NumStakeShares {
        assert!(
            self.total_staked_balance.as_attounc() > 0,
            "The total staked balance can't be 0"
        );
        UncToken::from_attounc((U256::from(self.total_stake_shares.as_attounc()) * U256::from(amount.as_attounc())
            / U256::from(self.total_staked_balance.as_attounc()))
        .as_u128())
    }

    /// Returns the number of "stake" shares rounded up corresponding to the given staked balance
    /// amount.
    ///
    /// Rounding up division of `a / b` is done using `(a + b - 1) / b`.
    pub(crate) fn num_shares_from_staked_amount_rounded_up(
        &self,
        amount: UncToken,
    ) -> NumStakeShares {
        assert!(
            self.total_staked_balance.as_attounc() > 0,
            "The total staked balance can't be 0"
        );
        UncToken::from_attounc(((U256::from(self.total_stake_shares.as_attounc()) * U256::from(amount.as_attounc())
            + U256::from(self.total_staked_balance.as_attounc() - 1))
            / U256::from(self.total_staked_balance.as_attounc()))
        .as_u128())
    }

    /// Returns the staked amount rounded down corresponding to the given number of "stake" shares.
    pub(crate) fn staked_amount_from_num_shares_rounded_down(
        &self,
        num_shares: NumStakeShares,
    ) -> UncToken {
        assert!(
            self.total_stake_shares.as_attounc() > 0,
            "The total number of stake shares can't be 0"
        );
        UncToken::from_attounc((U256::from(self.total_staked_balance.as_attounc()) * U256::from(num_shares.as_attounc())
            / U256::from(self.total_stake_shares.as_attounc()))
        .as_u128())
    }

    /// Returns the staked amount rounded up corresponding to the given number of "stake" shares.
    ///
    /// Rounding up division of `a / b` is done using `(a + b - 1) / b`.
    pub(crate) fn staked_amount_from_num_shares_rounded_up(
        &self,
        num_shares: NumStakeShares,
    ) -> UncToken {
        assert!(
            self.total_stake_shares.as_attounc() > 0,
            "The total number of stake shares can't be 0"
        );
        UncToken::from_attounc(((U256::from(self.total_staked_balance.as_attounc()) * U256::from(num_shares.as_attounc())
            + U256::from(self.total_stake_shares.as_attounc() - 1))
            / U256::from(self.total_stake_shares.as_attounc()))
        .as_u128())
    }

    /// Inner method to get the given account or a new default value account.
    pub(crate) fn internal_get_account(&self, account_id: &AccountId) -> Account {
        self.accounts.get(account_id).cloned().unwrap_or_default()
    }

    /// Inner method to save the given account for a given account ID.
    /// If the account balances are 0, the account is deleted instead to release storage.
    pub(crate) fn internal_save_account(&mut self, account_id: &AccountId, account: &Account) {
        if account.unstaked.as_attounc() > 0 || account.stake_shares.as_attounc() > 0 {
            self.accounts.insert(account_id.clone(), account.clone());
        } else {
            self.accounts.remove(account_id);
        }
    }
}
