mod types;
mod utils;

pub use crate::types::*;
use crate::utils::*;
use unc_sdk::json_types::U128;
use unc_sdk::{env, ext_contract, unc, AccountId, UncToken, Promise};

/// There is no deposit balance attached.
const NO_DEPOSIT: UncToken = UncToken::from_attounc(0);
const TRANSFERS_STARTED: u64 = 1602614338293769340; /* 13 October 2020 18:38:58.293 */

const CODE: &[u8] = include_bytes!("../../res/lockup_contract.wasm");

pub mod gas {
    use unc_sdk::Gas;

    /// The base amount of gas for a regular execution.
    const BASE: Gas = Gas::from_gas(25_000_000_000_000);

    /// The amount of Gas the contract will attach to the promise to create the lockup.
    pub const LOCKUP_NEW: Gas = BASE;

    /// The amount of Gas the contract will attach to the callback to itself.
    /// The base for the execution and the base for cash rollback.
    pub const CALLBACK: Gas = BASE;
}

const MIN_ATTACHED_BALANCE: u128 = 3_500_000_000_000_000_000_000_000;

/// External interface for the callbacks to self.
#[ext_contract(ext_self)]
pub trait ExtSelf {
    fn on_lockup_create(
        &mut self,
        lockup_account_id: AccountId,
        attached_deposit: U128,
        predecessor_account_id: AccountId,
    ) -> bool;
}

#[unc(contract_state)]
pub struct LockupFactory {
    whitelist_account_id: AccountId,
    foundation_account_id: AccountId,
}


#[unc(serializers=[json])]
pub struct LockupArgs {
    owner_account_id: AccountId,
    lockup_duration: WrappedDuration,
    lockup_timestamp: Option<WrappedTimestamp>,
    transfers_information: TransfersInformation,
    vesting_schedule: Option<VestingScheduleOrHash>,
    release_duration: Option<WrappedDuration>,
    staking_pool_whitelist_account_id: AccountId,
    foundation_account_id: Option<AccountId>,
}

impl Default for LockupFactory {
    fn default() -> Self {
        env::panic_str("LockupFactory should be initialized before usage")
    }
}

#[unc]
impl LockupFactory {
    #[init]
    pub fn new(
        whitelist_account_id: AccountId,
        foundation_account_id: AccountId,
    ) -> Self {
        assert!(!env::state_exists(), "The contract is already initialized");
        assert!(
            env::current_account_id().len() <= 23,
            "The account ID of this contract can't be more than 23 characters"
        );

        Self {
            whitelist_account_id: whitelist_account_id.into(),
            foundation_account_id: foundation_account_id.into(),
        }
    }

    /// Returns the foundation account id.
    pub fn get_foundation_account_id(&self) -> AccountId {
        self.foundation_account_id.clone()
    }

    /// Returns the lockup master account id.
    pub fn get_lockup_master_account_id(&self) -> AccountId {
        env::current_account_id()
    }

    /// Returns minimum attached balance.
    pub fn get_min_attached_balance(&self) -> U128 {
        MIN_ATTACHED_BALANCE.into()
    }

    #[payable]
    pub fn create(
        &mut self,
        owner_account_id: AccountId,
        lockup_duration: WrappedDuration,
        lockup_timestamp: Option<WrappedTimestamp>,
        vesting_schedule: Option<VestingScheduleOrHash>,
        release_duration: Option<WrappedDuration>,
        whitelist_account_id: Option<AccountId>,
    ) -> Promise {
        assert!(env::attached_deposit() >= UncToken::from_attounc(MIN_ATTACHED_BALANCE), "Not enough attached deposit");

        let byte_slice = env::sha256(owner_account_id.as_bytes());
        let lockup_account_id: AccountId =
            format!("{}.{}", hex::encode(&byte_slice[..20]), env::current_account_id()).parse().unwrap();

        let mut foundation_account: Option<AccountId> = None;
        if vesting_schedule.is_some() {
            foundation_account = Some(self.foundation_account_id.clone());
        };

        // Defaults to the whitelist account ID given on init call.
        let staking_pool_whitelist_account_id = if let Some(account_id) = whitelist_account_id {
            account_id.into()
        } else {
            self.whitelist_account_id.clone()
        };

        let transfers_enabled: WrappedTimestamp = TRANSFERS_STARTED.into();
        Promise::new(lockup_account_id.clone())
            .create_account()
            .deploy_contract(CODE.to_vec())
            .transfer(env::attached_deposit())
            .function_call(
                "new".to_string(),
                unc_sdk::serde_json::to_vec(&LockupArgs {
                    owner_account_id,
                    lockup_duration,
                    lockup_timestamp,
                    transfers_information: TransfersInformation::TransfersEnabled {
                        transfers_timestamp: transfers_enabled,
                    },
                    vesting_schedule,
                    release_duration,
                    staking_pool_whitelist_account_id,
                    foundation_account_id: foundation_account,
                })
                    .unwrap(),
                NO_DEPOSIT,
                gas::LOCKUP_NEW,
            )
            .then(ext_self::ext(env::current_account_id())
                .with_static_gas(gas::CALLBACK)
                .with_attached_deposit(NO_DEPOSIT)
                .on_lockup_create(
                    lockup_account_id,
                    env::attached_deposit().as_attounc().into(),
                    env::predecessor_account_id(),
            ))
    }

    /// Callback after a lockup was created.
    /// Returns the promise if the lockup creation succeeded.
    /// Otherwise refunds the attached deposit and returns `false`.
    pub fn on_lockup_create(
        &mut self,
        lockup_account_id: AccountId,
        attached_deposit: U128,
        predecessor_account_id: AccountId,
    ) -> bool {
        assert_self();

        let lockup_account_created = is_promise_success();

        if lockup_account_created {
            env::log_str(
                format!("The lockup contract {} was successfully created.", lockup_account_id)
                    .as_str(),
            );
            true
        } else {
            env::log_str(
                format!(
                    "The lockup {} creation has failed. Returning attached deposit of {} to {}",
                    lockup_account_id, attached_deposit.0, predecessor_account_id
                )
                    .as_str(),
            );
            Promise::new(predecessor_account_id).transfer(UncToken::from_attounc(attached_deposit.0));
            false
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    mod test_utils;

    use super::*;
    use unc_sdk::{testing_env, test_vm_config, RuntimeFeesConfig, PromiseResult};
    use unc_sdk::test_utils::VMContextBuilder;
    use test_utils::*;

    fn new_vesting_schedule(offset_in_days: u64) -> VestingSchedule {
        VestingSchedule {
            start_timestamp: to_ts(GENESIS_TIME_IN_DAYS - YEAR + offset_in_days).into(),
            cliff_timestamp: to_ts(GENESIS_TIME_IN_DAYS + offset_in_days).into(),
            end_timestamp: to_ts(GENESIS_TIME_IN_DAYS + YEAR * 3 + offset_in_days).into(),
        }
    }

    #[test]
    fn test_get_factory_vars() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let contract = LockupFactory::new(
            whitelist_account_id(),
            foundation_account_id(),
        );

        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .is_view(true)
            .build());

        assert_eq!(contract.get_min_attached_balance().0, MIN_ATTACHED_BALANCE);
        assert_eq!(
            contract.get_foundation_account_id(),
            foundation_account_id()
        );
        println!("{}", contract.get_lockup_master_account_id());
        assert_eq!(
            contract.get_lockup_master_account_id(),
            lockup_master_account_id()
        );
    }

    #[test]
    fn test_create_lockup_success() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let mut contract = LockupFactory::new(
            whitelist_account_id(),
            foundation_account_id(),
        );

        const LOCKUP_DURATION: u64 = 63036000000000000; /* 24 months */
        let lockup_duration: WrappedTimestamp = LOCKUP_DURATION.into();

        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_tokens_owner())
            .attached_deposit(UncToken::from_attounc(ntoy(35)))
            .is_view(false)
            .build());

        contract.create(account_tokens_owner(), lockup_duration, None, None, None, None);

        let context = VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_factory())
            .attached_deposit(UncToken::from_attounc(ntoy(0)))
            .is_view(false)
            .build();

        testing_env!(
            context.clone(),
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            vec![PromiseResult::Successful(vec![])],
        );
        println!("{}", lockup_account());
        contract.on_lockup_create(
            lockup_account(),
            ntoy(30).into(),
            account_tokens_owner(),
        );
    }

    #[test]
    fn test_create_lockup_with_vesting_success() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let mut contract = LockupFactory::new(
            whitelist_account_id(),
            foundation_account_id(),
        );

        const LOCKUP_DURATION: u64 = 63036000000000000; /* 24 months */
        const LOCKUP_TIMESTAMP: u64 = 1661990400000000000; /* 1 September 2022 00:00:00 */
        let lockup_duration: WrappedTimestamp = LOCKUP_DURATION.into();
        let lockup_timestamp: WrappedTimestamp = LOCKUP_TIMESTAMP.into();

        let vesting_schedule = Some(new_vesting_schedule(10));

        let vesting_schedule = vesting_schedule.map(|vesting_schedule| {
            VestingScheduleOrHash::VestingHash(
                VestingScheduleWithSalt { vesting_schedule, salt: SALT.to_vec().into() }
                    .hash()
                    .into(),
            )
        });

        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_tokens_owner())
            .attached_deposit(UncToken::from_attounc(ntoy(35)))
            .is_view(false)
            .build());

        contract.create(
            account_tokens_owner(),
            lockup_duration,
            Some(lockup_timestamp),
            vesting_schedule,
            None,
            None,
        );

        let context = VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_factory())
            .attached_deposit(UncToken::from_attounc(ntoy(0)))
            .build();

        testing_env!(
            context.clone(),
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            vec![PromiseResult::Successful(vec![])],
        );
        contract.on_lockup_create(
            lockup_account(),
            ntoy(30).into(),
            account_tokens_owner(),
        );
    }

    #[test]
    #[should_panic(expected = "Not enough attached deposit")]
    fn test_create_lockup_not_enough_deposit() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let mut contract = LockupFactory::new(
            whitelist_account_id(),
            foundation_account_id(),
        );

        const LOCKUP_DURATION: u64 = 63036000000000000; /* 24 months */
        let lockup_duration: WrappedTimestamp = LOCKUP_DURATION.into();

        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_tokens_owner())
            .attached_deposit(UncToken::from_attounc(ntoy(1))) /* Storage reduced to 3.5 UNC */
            .is_view(false)
            .build());

        contract.create(account_tokens_owner(), lockup_duration, None, None, None, None);
    }

    #[test]
    fn test_create_lockup_rollback() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let mut contract = LockupFactory::new(
            whitelist_account_id(),
            foundation_account_id(),
        );

        const LOCKUP_DURATION: u64 = 63036000000000000; /* 24 months */
        let lockup_duration: WrappedTimestamp = LOCKUP_DURATION.into();

        let context = VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_tokens_owner())
            .attached_deposit(UncToken::from_attounc(ntoy(35)))
            .is_view(false)
            .build();
        testing_env!(context.clone());

        contract.create(account_tokens_owner(), lockup_duration, None, None, None, None);

        let context = VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_factory())
            .attached_deposit(UncToken::from_attounc(ntoy(0)))
            .account_balance(context.account_balance.saturating_add(UncToken::from_attounc(ntoy(35))))
            .is_view(false)
            .build();

        testing_env!(
            context.clone(),
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            vec![PromiseResult::Failed],
        );

        let res = contract.on_lockup_create(
            lockup_account(),
            ntoy(35).into(),
            account_tokens_owner(),
        );

        match res {
            true => panic!("Unexpected result, should return false"),
            false => assert!(true),
        };
    }

    #[test]
    fn test_create_lockup_with_custom_whitelist_success() {
        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_unc())
            .build());

        let mut contract = LockupFactory::new(whitelist_account_id(), foundation_account_id());

        const LOCKUP_DURATION: u64 = 63036000000000000; /* 24 months */
        let lockup_duration: WrappedTimestamp = LOCKUP_DURATION.into();

        testing_env!(VMContextBuilder::new()
            .current_account_id(account_factory())
            .predecessor_account_id(account_tokens_owner())
            .attached_deposit(UncToken::from_attounc(ntoy(35)))
            .is_view(false)
            .build());

        contract.create(
            account_tokens_owner(),
            lockup_duration,
            None,
            None,
            None,
            Some(custom_whitelist_account_id()),
        );

        testing_env!(
            VMContextBuilder::new()
                .current_account_id(account_factory())
                .predecessor_account_id(account_factory())
                .attached_deposit(UncToken::from_attounc(ntoy(0)))
                .is_view(false)
                .build(),
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            vec![PromiseResult::Successful(vec![])],
        );

        println!("{}", lockup_account());
        contract.on_lockup_create(
            lockup_account(),
            ntoy(30).into(),
            account_tokens_owner(),
        );
    }
}
