use super::{DeviceId, DeviceObservation, DeviceObservationIdentity, ObservationId};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(crate) fn scan_device_observations() -> ObservationInventory {
    static SCANNER: OnceLock<Mutex<DeviceObservationScanner>> = OnceLock::new();

    let mut scanner = SCANNER
        .get_or_init(|| Mutex::new(DeviceObservationScanner::new()))
        .lock()
        .expect("production device observation scanner poisoned");
    scanner.scan_with(
        crate::ipod::device::candidate_mount_points(),
        super::observe_mount,
    )
}

pub struct DeviceObservationScanner {
    next_observation_id: u64,
    unavailable_by_mount: HashMap<PathBuf, ObservationId>,
}

impl DeviceObservationScanner {
    pub fn new() -> Self {
        Self {
            next_observation_id: 1,
            unavailable_by_mount: HashMap::new(),
        }
    }

    pub fn scan_with<I, P, O>(&mut self, candidates: I, mut observe: O) -> ObservationInventory
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
        O: FnMut(&Path, ObservationId) -> Option<DeviceObservation>,
    {
        let mut observations = Vec::new();
        let mut unavailable_by_mount = HashMap::new();

        for candidate in candidates {
            let mount_path = candidate.as_ref();
            let retained_observation_id = self.unavailable_by_mount.get(mount_path).cloned();
            let observation_id = retained_observation_id
                .clone()
                .unwrap_or_else(|| self.allocate_observation_id());

            if let Some(observation) = observe(mount_path, observation_id.clone()) {
                if observation.observation_id().is_some() || retained_observation_id.is_some() {
                    unavailable_by_mount.insert(mount_path.to_path_buf(), observation_id);
                }
                observations.push(observation);
            }
        }

        self.unavailable_by_mount = unavailable_by_mount;
        ObservationInventory::new(observations)
    }

    fn allocate_observation_id(&mut self) -> ObservationId {
        let id = ObservationId::new(self.next_observation_id);
        self.next_observation_id = self
            .next_observation_id
            .checked_add(1)
            .expect("device observation ID exhausted");
        id
    }
}

impl Default for DeviceObservationScanner {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ObservationInventory {
    observations: Vec<DeviceObservation>,
    duplicate_device_ids: BTreeSet<DeviceId>,
}

impl ObservationInventory {
    fn new(mut observations: Vec<DeviceObservation>) -> Self {
        observations.sort_by(|left, right| match (left.identity(), right.identity()) {
            (
                DeviceObservationIdentity::Identified(left_id),
                DeviceObservationIdentity::Identified(right_id),
            ) => left_id
                .cmp(right_id)
                .then_with(|| left.mount_path().cmp(right.mount_path())),
            (
                DeviceObservationIdentity::Unavailable(left_id),
                DeviceObservationIdentity::Unavailable(right_id),
            ) => left_id
                .cmp(right_id)
                .then_with(|| left.mount_path().cmp(right.mount_path())),
            (DeviceObservationIdentity::Identified(_), _) => std::cmp::Ordering::Less,
            (DeviceObservationIdentity::Unavailable(_), _) => std::cmp::Ordering::Greater,
        });

        let mut seen = BTreeSet::new();
        let mut duplicate_device_ids = BTreeSet::new();
        for device_id in observations.iter().filter_map(DeviceObservation::device_id) {
            if !seen.insert(device_id.clone()) {
                duplicate_device_ids.insert(device_id.clone());
            }
        }

        Self {
            observations,
            duplicate_device_ids,
        }
    }

    pub fn observations(&self) -> &[DeviceObservation] {
        &self.observations
    }

    /// Returns true only for the exact observation stored in this completed
    /// inventory and judged eligible against its full conflict set.
    pub fn is_uniquely_mutation_eligible(&self, observation: &DeviceObservation) -> bool {
        self.observations
            .iter()
            .any(|member| std::ptr::eq(member, observation))
            && observation.is_mutation_eligible()
            && observation
                .device_id()
                .is_some_and(|device_id| !self.duplicate_device_ids.contains(device_id))
    }
}
