//! Constants for the OIDs Nocturne polls.
//!
//! One constant per MIB object, named after the object in `SCREAMING_SNAKE_CASE`
//! (`ifHCInOctets` → [`IF_HC_IN_OCTETS`]):
//!
//! * **scalars** include their `.0` instance and can be passed straight to `Client::get`
//!   ([`SYS_NAME`] is `"1.3.6.1.2.1.1.5.0"`);
//! * **columns** are the column OID, to be walked, used with `Client::table`, or completed
//!   with a row index ([`IF_HC_IN_OCTETS`] is `"1.3.6.1.2.1.31.1.1.1.6"`);
//! * **tables** (`…_TABLE`) are the table OID, for walking a table whole.
//!
//! ```
//! use nocturne_snmp::{Oid, names, oids};
//!
//! let column: Oid = oids::IF_HC_IN_OCTETS.parse().unwrap();
//! assert_eq!(names::describe(&column.child(3)).as_deref(), Some("ifHCInOctets.3"));
//! assert_eq!(names::resolve("sysName.0"), oids::SYS_NAME.parse().ok());
//! ```
//!
//! Covered: the system group, IF-MIB (`ifTable`, `ifXTable`, `ifStackTable`), the IP-MIB
//! address and ARP/neighbour tables, HOST-RESOURCES-MIB, ENTITY-MIB and ENTITY-SENSOR-MIB,
//! BRIDGE-MIB and Q-BRIDGE-MIB, LLDP-MIB and CISCO-CDP-MIB, and the CPU / memory /
//! temperature objects of net-snmp (UCD-SNMP-MIB, LM-SENSORS-MIB), Cisco, MikroTik, Juniper,
//! Arista, Huawei, HPE/Aruba (ProCurve, Comware, ArubaOS-CX, ArubaOS), Fortinet, Palo Alto and
//! Ubiquiti. Arista, Palo Alto and MikroTik report CPU load and memory through
//! HOST-RESOURCES-MIB ([`HR_PROCESSOR_LOAD`], [`HR_STORAGE_USED`]) and temperatures through
//! ENTITY-SENSOR-MIB ([`ENT_PHY_SENSOR_VALUE`]) or their own tables below.
//!
//! Every entry was taken from the defining MIB module and cross-checked with net-snmp's MIB
//! parser. Objects whose MIB names are too generic to stand alone carry a vendor prefix:
//! `FROGFOOT_…` (Ubiquiti airOS: [`FROGFOOT_MEM_TOTAL`], [`FROGFOOT_LOAD_VALUE`]),
//! `CISCO_…` for the old CPU scalars ([`CISCO_AVG_BUSY5`]), `EDGESWITCH_…`, `WLSX_…`, and
//! [`PAN_IF_INDEX`] / [`PAN_IF_DESCR`] (Palo Alto's own `ifIndex` and `ifDescr`).

/// Declares the constants and, from the same lines, the table `names` is built on.
macro_rules! objects {
    ($(
        $module:literal {
            $( $constant:ident = $name:literal $oid:literal $($kind:ident)? ; )*
        }
    )*) => {
        $($(
            #[doc = concat!("`", $module, "::", $name, "`")]
            pub const $constant: &str = objects!(@instance $oid $($kind)?);
        )*)*

        /// Every object above as `(module, name, OID)`; the OID is the object's own, without
        /// an instance suffix.
        pub(crate) static OBJECTS: &[(&str, &str, &str)] = &[
            $($( ($module, $name, $oid), )*)*
        ];
    };
    (@instance $oid:literal scalar) => { concat!($oid, ".0") };
    (@instance $oid:literal) => { $oid };
}

objects! {
    "SNMPv2-MIB" {
        SYS_DESCR = "sysDescr" "1.3.6.1.2.1.1.1" scalar;
        SYS_OBJECT_ID = "sysObjectID" "1.3.6.1.2.1.1.2" scalar;
        SYS_UP_TIME = "sysUpTime" "1.3.6.1.2.1.1.3" scalar;
        SYS_CONTACT = "sysContact" "1.3.6.1.2.1.1.4" scalar;
        SYS_NAME = "sysName" "1.3.6.1.2.1.1.5" scalar;
        SYS_LOCATION = "sysLocation" "1.3.6.1.2.1.1.6" scalar;
        SYS_SERVICES = "sysServices" "1.3.6.1.2.1.1.7" scalar;
        SYS_OR_LAST_CHANGE = "sysORLastChange" "1.3.6.1.2.1.1.8" scalar;
        SYS_OR_TABLE = "sysORTable" "1.3.6.1.2.1.1.9";
        SYS_ORID = "sysORID" "1.3.6.1.2.1.1.9.1.2";
        SYS_OR_DESCR = "sysORDescr" "1.3.6.1.2.1.1.9.1.3";
        SYS_OR_UP_TIME = "sysORUpTime" "1.3.6.1.2.1.1.9.1.4";
    }
    "IF-MIB" {
        IF_NUMBER = "ifNumber" "1.3.6.1.2.1.2.1" scalar;
        IF_TABLE = "ifTable" "1.3.6.1.2.1.2.2";
        IF_INDEX = "ifIndex" "1.3.6.1.2.1.2.2.1.1";
        IF_DESCR = "ifDescr" "1.3.6.1.2.1.2.2.1.2";
        IF_TYPE = "ifType" "1.3.6.1.2.1.2.2.1.3";
        IF_MTU = "ifMtu" "1.3.6.1.2.1.2.2.1.4";
        IF_SPEED = "ifSpeed" "1.3.6.1.2.1.2.2.1.5";
        IF_PHYS_ADDRESS = "ifPhysAddress" "1.3.6.1.2.1.2.2.1.6";
        IF_ADMIN_STATUS = "ifAdminStatus" "1.3.6.1.2.1.2.2.1.7";
        IF_OPER_STATUS = "ifOperStatus" "1.3.6.1.2.1.2.2.1.8";
        IF_LAST_CHANGE = "ifLastChange" "1.3.6.1.2.1.2.2.1.9";
        IF_IN_OCTETS = "ifInOctets" "1.3.6.1.2.1.2.2.1.10";
        IF_IN_UCAST_PKTS = "ifInUcastPkts" "1.3.6.1.2.1.2.2.1.11";
        IF_IN_N_UCAST_PKTS = "ifInNUcastPkts" "1.3.6.1.2.1.2.2.1.12";
        IF_IN_DISCARDS = "ifInDiscards" "1.3.6.1.2.1.2.2.1.13";
        IF_IN_ERRORS = "ifInErrors" "1.3.6.1.2.1.2.2.1.14";
        IF_IN_UNKNOWN_PROTOS = "ifInUnknownProtos" "1.3.6.1.2.1.2.2.1.15";
        IF_OUT_OCTETS = "ifOutOctets" "1.3.6.1.2.1.2.2.1.16";
        IF_OUT_UCAST_PKTS = "ifOutUcastPkts" "1.3.6.1.2.1.2.2.1.17";
        IF_OUT_N_UCAST_PKTS = "ifOutNUcastPkts" "1.3.6.1.2.1.2.2.1.18";
        IF_OUT_DISCARDS = "ifOutDiscards" "1.3.6.1.2.1.2.2.1.19";
        IF_OUT_ERRORS = "ifOutErrors" "1.3.6.1.2.1.2.2.1.20";
        IF_OUT_Q_LEN = "ifOutQLen" "1.3.6.1.2.1.2.2.1.21";
        IF_SPECIFIC = "ifSpecific" "1.3.6.1.2.1.2.2.1.22";
        IF_X_TABLE = "ifXTable" "1.3.6.1.2.1.31.1.1";
        IF_NAME = "ifName" "1.3.6.1.2.1.31.1.1.1.1";
        IF_IN_MULTICAST_PKTS = "ifInMulticastPkts" "1.3.6.1.2.1.31.1.1.1.2";
        IF_IN_BROADCAST_PKTS = "ifInBroadcastPkts" "1.3.6.1.2.1.31.1.1.1.3";
        IF_OUT_MULTICAST_PKTS = "ifOutMulticastPkts" "1.3.6.1.2.1.31.1.1.1.4";
        IF_OUT_BROADCAST_PKTS = "ifOutBroadcastPkts" "1.3.6.1.2.1.31.1.1.1.5";
        IF_HC_IN_OCTETS = "ifHCInOctets" "1.3.6.1.2.1.31.1.1.1.6";
        IF_HC_IN_UCAST_PKTS = "ifHCInUcastPkts" "1.3.6.1.2.1.31.1.1.1.7";
        IF_HC_IN_MULTICAST_PKTS = "ifHCInMulticastPkts" "1.3.6.1.2.1.31.1.1.1.8";
        IF_HC_IN_BROADCAST_PKTS = "ifHCInBroadcastPkts" "1.3.6.1.2.1.31.1.1.1.9";
        IF_HC_OUT_OCTETS = "ifHCOutOctets" "1.3.6.1.2.1.31.1.1.1.10";
        IF_HC_OUT_UCAST_PKTS = "ifHCOutUcastPkts" "1.3.6.1.2.1.31.1.1.1.11";
        IF_HC_OUT_MULTICAST_PKTS = "ifHCOutMulticastPkts" "1.3.6.1.2.1.31.1.1.1.12";
        IF_HC_OUT_BROADCAST_PKTS = "ifHCOutBroadcastPkts" "1.3.6.1.2.1.31.1.1.1.13";
        IF_LINK_UP_DOWN_TRAP_ENABLE = "ifLinkUpDownTrapEnable" "1.3.6.1.2.1.31.1.1.1.14";
        IF_HIGH_SPEED = "ifHighSpeed" "1.3.6.1.2.1.31.1.1.1.15";
        IF_PROMISCUOUS_MODE = "ifPromiscuousMode" "1.3.6.1.2.1.31.1.1.1.16";
        IF_CONNECTOR_PRESENT = "ifConnectorPresent" "1.3.6.1.2.1.31.1.1.1.17";
        IF_ALIAS = "ifAlias" "1.3.6.1.2.1.31.1.1.1.18";
        IF_COUNTER_DISCONTINUITY_TIME = "ifCounterDiscontinuityTime" "1.3.6.1.2.1.31.1.1.1.19";
        IF_STACK_TABLE = "ifStackTable" "1.3.6.1.2.1.31.1.2";
        IF_STACK_STATUS = "ifStackStatus" "1.3.6.1.2.1.31.1.2.1.3";
        IF_TABLE_LAST_CHANGE = "ifTableLastChange" "1.3.6.1.2.1.31.1.5" scalar;
        IF_STACK_LAST_CHANGE = "ifStackLastChange" "1.3.6.1.2.1.31.1.6" scalar;
    }
    "IP-MIB" {
        IP_FORWARDING = "ipForwarding" "1.3.6.1.2.1.4.1" scalar;
        IP_DEFAULT_TTL = "ipDefaultTTL" "1.3.6.1.2.1.4.2" scalar;
        IP_ADDR_TABLE = "ipAddrTable" "1.3.6.1.2.1.4.20";
        IP_AD_ENT_ADDR = "ipAdEntAddr" "1.3.6.1.2.1.4.20.1.1";
        IP_AD_ENT_IF_INDEX = "ipAdEntIfIndex" "1.3.6.1.2.1.4.20.1.2";
        IP_AD_ENT_NET_MASK = "ipAdEntNetMask" "1.3.6.1.2.1.4.20.1.3";
        IP_AD_ENT_BCAST_ADDR = "ipAdEntBcastAddr" "1.3.6.1.2.1.4.20.1.4";
        IP_AD_ENT_REASM_MAX_SIZE = "ipAdEntReasmMaxSize" "1.3.6.1.2.1.4.20.1.5";
        IP_NET_TO_MEDIA_TABLE = "ipNetToMediaTable" "1.3.6.1.2.1.4.22";
        IP_NET_TO_MEDIA_IF_INDEX = "ipNetToMediaIfIndex" "1.3.6.1.2.1.4.22.1.1";
        IP_NET_TO_MEDIA_PHYS_ADDRESS = "ipNetToMediaPhysAddress" "1.3.6.1.2.1.4.22.1.2";
        IP_NET_TO_MEDIA_NET_ADDRESS = "ipNetToMediaNetAddress" "1.3.6.1.2.1.4.22.1.3";
        IP_NET_TO_MEDIA_TYPE = "ipNetToMediaType" "1.3.6.1.2.1.4.22.1.4";
        IP_ADDRESS_TABLE = "ipAddressTable" "1.3.6.1.2.1.4.34";
        IP_ADDRESS_IF_INDEX = "ipAddressIfIndex" "1.3.6.1.2.1.4.34.1.3";
        IP_ADDRESS_TYPE = "ipAddressType" "1.3.6.1.2.1.4.34.1.4";
        IP_ADDRESS_PREFIX = "ipAddressPrefix" "1.3.6.1.2.1.4.34.1.5";
        IP_ADDRESS_ORIGIN = "ipAddressOrigin" "1.3.6.1.2.1.4.34.1.6";
        IP_ADDRESS_STATUS = "ipAddressStatus" "1.3.6.1.2.1.4.34.1.7";
        IP_ADDRESS_CREATED = "ipAddressCreated" "1.3.6.1.2.1.4.34.1.8";
        IP_ADDRESS_LAST_CHANGED = "ipAddressLastChanged" "1.3.6.1.2.1.4.34.1.9";
        IP_ADDRESS_ROW_STATUS = "ipAddressRowStatus" "1.3.6.1.2.1.4.34.1.10";
        IP_ADDRESS_STORAGE_TYPE = "ipAddressStorageType" "1.3.6.1.2.1.4.34.1.11";
        IP_NET_TO_PHYSICAL_TABLE = "ipNetToPhysicalTable" "1.3.6.1.2.1.4.35";
        IP_NET_TO_PHYSICAL_PHYS_ADDRESS = "ipNetToPhysicalPhysAddress" "1.3.6.1.2.1.4.35.1.4";
        IP_NET_TO_PHYSICAL_LAST_UPDATED = "ipNetToPhysicalLastUpdated" "1.3.6.1.2.1.4.35.1.5";
        IP_NET_TO_PHYSICAL_TYPE = "ipNetToPhysicalType" "1.3.6.1.2.1.4.35.1.6";
        IP_NET_TO_PHYSICAL_STATE = "ipNetToPhysicalState" "1.3.6.1.2.1.4.35.1.7";
        IP_NET_TO_PHYSICAL_ROW_STATUS = "ipNetToPhysicalRowStatus" "1.3.6.1.2.1.4.35.1.8";
    }
    "HOST-RESOURCES-MIB" {
        HR_SYSTEM_UPTIME = "hrSystemUptime" "1.3.6.1.2.1.25.1.1" scalar;
        HR_SYSTEM_DATE = "hrSystemDate" "1.3.6.1.2.1.25.1.2" scalar;
        HR_SYSTEM_INITIAL_LOAD_DEVICE = "hrSystemInitialLoadDevice" "1.3.6.1.2.1.25.1.3" scalar;
        HR_SYSTEM_INITIAL_LOAD_PARAMETERS = "hrSystemInitialLoadParameters" "1.3.6.1.2.1.25.1.4" scalar;
        HR_SYSTEM_NUM_USERS = "hrSystemNumUsers" "1.3.6.1.2.1.25.1.5" scalar;
        HR_SYSTEM_PROCESSES = "hrSystemProcesses" "1.3.6.1.2.1.25.1.6" scalar;
        HR_SYSTEM_MAX_PROCESSES = "hrSystemMaxProcesses" "1.3.6.1.2.1.25.1.7" scalar;
        HR_MEMORY_SIZE = "hrMemorySize" "1.3.6.1.2.1.25.2.2" scalar;
        HR_STORAGE_TABLE = "hrStorageTable" "1.3.6.1.2.1.25.2.3";
        HR_STORAGE_INDEX = "hrStorageIndex" "1.3.6.1.2.1.25.2.3.1.1";
        HR_STORAGE_TYPE = "hrStorageType" "1.3.6.1.2.1.25.2.3.1.2";
        HR_STORAGE_DESCR = "hrStorageDescr" "1.3.6.1.2.1.25.2.3.1.3";
        HR_STORAGE_ALLOCATION_UNITS = "hrStorageAllocationUnits" "1.3.6.1.2.1.25.2.3.1.4";
        HR_STORAGE_SIZE = "hrStorageSize" "1.3.6.1.2.1.25.2.3.1.5";
        HR_STORAGE_USED = "hrStorageUsed" "1.3.6.1.2.1.25.2.3.1.6";
        HR_STORAGE_ALLOCATION_FAILURES = "hrStorageAllocationFailures" "1.3.6.1.2.1.25.2.3.1.7";
        HR_DEVICE_TABLE = "hrDeviceTable" "1.3.6.1.2.1.25.3.2";
        HR_DEVICE_INDEX = "hrDeviceIndex" "1.3.6.1.2.1.25.3.2.1.1";
        HR_DEVICE_TYPE = "hrDeviceType" "1.3.6.1.2.1.25.3.2.1.2";
        HR_DEVICE_DESCR = "hrDeviceDescr" "1.3.6.1.2.1.25.3.2.1.3";
        HR_DEVICE_ID = "hrDeviceID" "1.3.6.1.2.1.25.3.2.1.4";
        HR_DEVICE_STATUS = "hrDeviceStatus" "1.3.6.1.2.1.25.3.2.1.5";
        HR_DEVICE_ERRORS = "hrDeviceErrors" "1.3.6.1.2.1.25.3.2.1.6";
        HR_PROCESSOR_TABLE = "hrProcessorTable" "1.3.6.1.2.1.25.3.3";
        HR_PROCESSOR_FRW_ID = "hrProcessorFrwID" "1.3.6.1.2.1.25.3.3.1.1";
        HR_PROCESSOR_LOAD = "hrProcessorLoad" "1.3.6.1.2.1.25.3.3.1.2";
        HR_NETWORK_TABLE = "hrNetworkTable" "1.3.6.1.2.1.25.3.4";
        HR_NETWORK_IF_INDEX = "hrNetworkIfIndex" "1.3.6.1.2.1.25.3.4.1.1";
        HR_PRINTER_TABLE = "hrPrinterTable" "1.3.6.1.2.1.25.3.5";
        HR_PRINTER_STATUS = "hrPrinterStatus" "1.3.6.1.2.1.25.3.5.1.1";
        HR_PRINTER_DETECTED_ERROR_STATE = "hrPrinterDetectedErrorState" "1.3.6.1.2.1.25.3.5.1.2";
        HR_DISK_STORAGE_TABLE = "hrDiskStorageTable" "1.3.6.1.2.1.25.3.6";
        HR_DISK_STORAGE_ACCESS = "hrDiskStorageAccess" "1.3.6.1.2.1.25.3.6.1.1";
        HR_DISK_STORAGE_MEDIA = "hrDiskStorageMedia" "1.3.6.1.2.1.25.3.6.1.2";
        HR_DISK_STORAGE_REMOVEBLE = "hrDiskStorageRemoveble" "1.3.6.1.2.1.25.3.6.1.3";
        HR_DISK_STORAGE_CAPACITY = "hrDiskStorageCapacity" "1.3.6.1.2.1.25.3.6.1.4";
        HR_PARTITION_TABLE = "hrPartitionTable" "1.3.6.1.2.1.25.3.7";
        HR_PARTITION_INDEX = "hrPartitionIndex" "1.3.6.1.2.1.25.3.7.1.1";
        HR_PARTITION_LABEL = "hrPartitionLabel" "1.3.6.1.2.1.25.3.7.1.2";
        HR_PARTITION_ID = "hrPartitionID" "1.3.6.1.2.1.25.3.7.1.3";
        HR_PARTITION_SIZE = "hrPartitionSize" "1.3.6.1.2.1.25.3.7.1.4";
        HR_PARTITION_FS_INDEX = "hrPartitionFSIndex" "1.3.6.1.2.1.25.3.7.1.5";
        HR_FS_TABLE = "hrFSTable" "1.3.6.1.2.1.25.3.8";
        HR_FS_INDEX = "hrFSIndex" "1.3.6.1.2.1.25.3.8.1.1";
        HR_FS_MOUNT_POINT = "hrFSMountPoint" "1.3.6.1.2.1.25.3.8.1.2";
        HR_FS_REMOTE_MOUNT_POINT = "hrFSRemoteMountPoint" "1.3.6.1.2.1.25.3.8.1.3";
        HR_FS_TYPE = "hrFSType" "1.3.6.1.2.1.25.3.8.1.4";
        HR_FS_ACCESS = "hrFSAccess" "1.3.6.1.2.1.25.3.8.1.5";
        HR_FS_BOOTABLE = "hrFSBootable" "1.3.6.1.2.1.25.3.8.1.6";
        HR_FS_STORAGE_INDEX = "hrFSStorageIndex" "1.3.6.1.2.1.25.3.8.1.7";
        HR_FS_LAST_FULL_BACKUP_DATE = "hrFSLastFullBackupDate" "1.3.6.1.2.1.25.3.8.1.8";
        HR_FS_LAST_PARTIAL_BACKUP_DATE = "hrFSLastPartialBackupDate" "1.3.6.1.2.1.25.3.8.1.9";
        HR_SWOS_INDEX = "hrSWOSIndex" "1.3.6.1.2.1.25.4.1" scalar;
        HR_SW_RUN_TABLE = "hrSWRunTable" "1.3.6.1.2.1.25.4.2";
        HR_SW_RUN_INDEX = "hrSWRunIndex" "1.3.6.1.2.1.25.4.2.1.1";
        HR_SW_RUN_NAME = "hrSWRunName" "1.3.6.1.2.1.25.4.2.1.2";
        HR_SW_RUN_ID = "hrSWRunID" "1.3.6.1.2.1.25.4.2.1.3";
        HR_SW_RUN_PATH = "hrSWRunPath" "1.3.6.1.2.1.25.4.2.1.4";
        HR_SW_RUN_PARAMETERS = "hrSWRunParameters" "1.3.6.1.2.1.25.4.2.1.5";
        HR_SW_RUN_TYPE = "hrSWRunType" "1.3.6.1.2.1.25.4.2.1.6";
        HR_SW_RUN_STATUS = "hrSWRunStatus" "1.3.6.1.2.1.25.4.2.1.7";
        HR_SW_RUN_PERF_TABLE = "hrSWRunPerfTable" "1.3.6.1.2.1.25.5.1";
        HR_SW_RUN_PERF_CPU = "hrSWRunPerfCPU" "1.3.6.1.2.1.25.5.1.1.1";
        HR_SW_RUN_PERF_MEM = "hrSWRunPerfMem" "1.3.6.1.2.1.25.5.1.1.2";
    }
    "ENTITY-MIB" {
        ENT_PHYSICAL_TABLE = "entPhysicalTable" "1.3.6.1.2.1.47.1.1.1";
        ENT_PHYSICAL_DESCR = "entPhysicalDescr" "1.3.6.1.2.1.47.1.1.1.1.2";
        ENT_PHYSICAL_VENDOR_TYPE = "entPhysicalVendorType" "1.3.6.1.2.1.47.1.1.1.1.3";
        ENT_PHYSICAL_CONTAINED_IN = "entPhysicalContainedIn" "1.3.6.1.2.1.47.1.1.1.1.4";
        ENT_PHYSICAL_CLASS = "entPhysicalClass" "1.3.6.1.2.1.47.1.1.1.1.5";
        ENT_PHYSICAL_PARENT_REL_POS = "entPhysicalParentRelPos" "1.3.6.1.2.1.47.1.1.1.1.6";
        ENT_PHYSICAL_NAME = "entPhysicalName" "1.3.6.1.2.1.47.1.1.1.1.7";
        ENT_PHYSICAL_HARDWARE_REV = "entPhysicalHardwareRev" "1.3.6.1.2.1.47.1.1.1.1.8";
        ENT_PHYSICAL_FIRMWARE_REV = "entPhysicalFirmwareRev" "1.3.6.1.2.1.47.1.1.1.1.9";
        ENT_PHYSICAL_SOFTWARE_REV = "entPhysicalSoftwareRev" "1.3.6.1.2.1.47.1.1.1.1.10";
        ENT_PHYSICAL_SERIAL_NUM = "entPhysicalSerialNum" "1.3.6.1.2.1.47.1.1.1.1.11";
        ENT_PHYSICAL_MFG_NAME = "entPhysicalMfgName" "1.3.6.1.2.1.47.1.1.1.1.12";
        ENT_PHYSICAL_MODEL_NAME = "entPhysicalModelName" "1.3.6.1.2.1.47.1.1.1.1.13";
        ENT_PHYSICAL_ALIAS = "entPhysicalAlias" "1.3.6.1.2.1.47.1.1.1.1.14";
        ENT_PHYSICAL_ASSET_ID = "entPhysicalAssetID" "1.3.6.1.2.1.47.1.1.1.1.15";
        ENT_PHYSICAL_IS_FRU = "entPhysicalIsFRU" "1.3.6.1.2.1.47.1.1.1.1.16";
        ENT_PHYSICAL_MFG_DATE = "entPhysicalMfgDate" "1.3.6.1.2.1.47.1.1.1.1.17";
        ENT_PHYSICAL_URIS = "entPhysicalUris" "1.3.6.1.2.1.47.1.1.1.1.18";
        ENT_PHYSICAL_UUID = "entPhysicalUUID" "1.3.6.1.2.1.47.1.1.1.1.19";
        ENT_ALIAS_MAPPING_TABLE = "entAliasMappingTable" "1.3.6.1.2.1.47.1.3.2";
        ENT_ALIAS_MAPPING_IDENTIFIER = "entAliasMappingIdentifier" "1.3.6.1.2.1.47.1.3.2.1.2";
        ENT_LAST_CHANGE_TIME = "entLastChangeTime" "1.3.6.1.2.1.47.1.4.1" scalar;
    }
    "ENTITY-SENSOR-MIB" {
        ENT_PHY_SENSOR_TABLE = "entPhySensorTable" "1.3.6.1.2.1.99.1.1";
        ENT_PHY_SENSOR_TYPE = "entPhySensorType" "1.3.6.1.2.1.99.1.1.1.1";
        ENT_PHY_SENSOR_SCALE = "entPhySensorScale" "1.3.6.1.2.1.99.1.1.1.2";
        ENT_PHY_SENSOR_PRECISION = "entPhySensorPrecision" "1.3.6.1.2.1.99.1.1.1.3";
        ENT_PHY_SENSOR_VALUE = "entPhySensorValue" "1.3.6.1.2.1.99.1.1.1.4";
        ENT_PHY_SENSOR_OPER_STATUS = "entPhySensorOperStatus" "1.3.6.1.2.1.99.1.1.1.5";
        ENT_PHY_SENSOR_UNITS_DISPLAY = "entPhySensorUnitsDisplay" "1.3.6.1.2.1.99.1.1.1.6";
        ENT_PHY_SENSOR_VALUE_TIME_STAMP = "entPhySensorValueTimeStamp" "1.3.6.1.2.1.99.1.1.1.7";
        ENT_PHY_SENSOR_VALUE_UPDATE_RATE = "entPhySensorValueUpdateRate" "1.3.6.1.2.1.99.1.1.1.8";
    }
    "BRIDGE-MIB" {
        DOT1D_BASE_BRIDGE_ADDRESS = "dot1dBaseBridgeAddress" "1.3.6.1.2.1.17.1.1" scalar;
        DOT1D_BASE_NUM_PORTS = "dot1dBaseNumPorts" "1.3.6.1.2.1.17.1.2" scalar;
        DOT1D_BASE_TYPE = "dot1dBaseType" "1.3.6.1.2.1.17.1.3" scalar;
        DOT1D_BASE_PORT_TABLE = "dot1dBasePortTable" "1.3.6.1.2.1.17.1.4";
        DOT1D_BASE_PORT = "dot1dBasePort" "1.3.6.1.2.1.17.1.4.1.1";
        DOT1D_BASE_PORT_IF_INDEX = "dot1dBasePortIfIndex" "1.3.6.1.2.1.17.1.4.1.2";
        DOT1D_BASE_PORT_CIRCUIT = "dot1dBasePortCircuit" "1.3.6.1.2.1.17.1.4.1.3";
        DOT1D_BASE_PORT_DELAY_EXCEEDED_DISCARDS = "dot1dBasePortDelayExceededDiscards" "1.3.6.1.2.1.17.1.4.1.4";
        DOT1D_BASE_PORT_MTU_EXCEEDED_DISCARDS = "dot1dBasePortMtuExceededDiscards" "1.3.6.1.2.1.17.1.4.1.5";
        DOT1D_STP_PROTOCOL_SPECIFICATION = "dot1dStpProtocolSpecification" "1.3.6.1.2.1.17.2.1" scalar;
        DOT1D_STP_PRIORITY = "dot1dStpPriority" "1.3.6.1.2.1.17.2.2" scalar;
        DOT1D_STP_TIME_SINCE_TOPOLOGY_CHANGE = "dot1dStpTimeSinceTopologyChange" "1.3.6.1.2.1.17.2.3" scalar;
        DOT1D_STP_TOP_CHANGES = "dot1dStpTopChanges" "1.3.6.1.2.1.17.2.4" scalar;
        DOT1D_STP_DESIGNATED_ROOT = "dot1dStpDesignatedRoot" "1.3.6.1.2.1.17.2.5" scalar;
        DOT1D_STP_ROOT_COST = "dot1dStpRootCost" "1.3.6.1.2.1.17.2.6" scalar;
        DOT1D_STP_ROOT_PORT = "dot1dStpRootPort" "1.3.6.1.2.1.17.2.7" scalar;
        DOT1D_STP_MAX_AGE = "dot1dStpMaxAge" "1.3.6.1.2.1.17.2.8" scalar;
        DOT1D_STP_HELLO_TIME = "dot1dStpHelloTime" "1.3.6.1.2.1.17.2.9" scalar;
        DOT1D_STP_HOLD_TIME = "dot1dStpHoldTime" "1.3.6.1.2.1.17.2.10" scalar;
        DOT1D_STP_FORWARD_DELAY = "dot1dStpForwardDelay" "1.3.6.1.2.1.17.2.11" scalar;
        DOT1D_STP_BRIDGE_MAX_AGE = "dot1dStpBridgeMaxAge" "1.3.6.1.2.1.17.2.12" scalar;
        DOT1D_STP_BRIDGE_HELLO_TIME = "dot1dStpBridgeHelloTime" "1.3.6.1.2.1.17.2.13" scalar;
        DOT1D_STP_BRIDGE_FORWARD_DELAY = "dot1dStpBridgeForwardDelay" "1.3.6.1.2.1.17.2.14" scalar;
        DOT1D_STP_PORT_TABLE = "dot1dStpPortTable" "1.3.6.1.2.1.17.2.15";
        DOT1D_STP_PORT = "dot1dStpPort" "1.3.6.1.2.1.17.2.15.1.1";
        DOT1D_STP_PORT_PRIORITY = "dot1dStpPortPriority" "1.3.6.1.2.1.17.2.15.1.2";
        DOT1D_STP_PORT_STATE = "dot1dStpPortState" "1.3.6.1.2.1.17.2.15.1.3";
        DOT1D_STP_PORT_ENABLE = "dot1dStpPortEnable" "1.3.6.1.2.1.17.2.15.1.4";
        DOT1D_STP_PORT_PATH_COST = "dot1dStpPortPathCost" "1.3.6.1.2.1.17.2.15.1.5";
        DOT1D_STP_PORT_DESIGNATED_ROOT = "dot1dStpPortDesignatedRoot" "1.3.6.1.2.1.17.2.15.1.6";
        DOT1D_STP_PORT_DESIGNATED_COST = "dot1dStpPortDesignatedCost" "1.3.6.1.2.1.17.2.15.1.7";
        DOT1D_STP_PORT_DESIGNATED_BRIDGE = "dot1dStpPortDesignatedBridge" "1.3.6.1.2.1.17.2.15.1.8";
        DOT1D_STP_PORT_DESIGNATED_PORT = "dot1dStpPortDesignatedPort" "1.3.6.1.2.1.17.2.15.1.9";
        DOT1D_STP_PORT_FORWARD_TRANSITIONS = "dot1dStpPortForwardTransitions" "1.3.6.1.2.1.17.2.15.1.10";
        DOT1D_STP_PORT_PATH_COST32 = "dot1dStpPortPathCost32" "1.3.6.1.2.1.17.2.15.1.11";
        DOT1D_TP_LEARNED_ENTRY_DISCARDS = "dot1dTpLearnedEntryDiscards" "1.3.6.1.2.1.17.4.1" scalar;
        DOT1D_TP_AGING_TIME = "dot1dTpAgingTime" "1.3.6.1.2.1.17.4.2" scalar;
        DOT1D_TP_FDB_TABLE = "dot1dTpFdbTable" "1.3.6.1.2.1.17.4.3";
        DOT1D_TP_FDB_ADDRESS = "dot1dTpFdbAddress" "1.3.6.1.2.1.17.4.3.1.1";
        DOT1D_TP_FDB_PORT = "dot1dTpFdbPort" "1.3.6.1.2.1.17.4.3.1.2";
        DOT1D_TP_FDB_STATUS = "dot1dTpFdbStatus" "1.3.6.1.2.1.17.4.3.1.3";
        DOT1D_TP_PORT_TABLE = "dot1dTpPortTable" "1.3.6.1.2.1.17.4.4";
        DOT1D_TP_PORT = "dot1dTpPort" "1.3.6.1.2.1.17.4.4.1.1";
        DOT1D_TP_PORT_MAX_INFO = "dot1dTpPortMaxInfo" "1.3.6.1.2.1.17.4.4.1.2";
        DOT1D_TP_PORT_IN_FRAMES = "dot1dTpPortInFrames" "1.3.6.1.2.1.17.4.4.1.3";
        DOT1D_TP_PORT_OUT_FRAMES = "dot1dTpPortOutFrames" "1.3.6.1.2.1.17.4.4.1.4";
        DOT1D_TP_PORT_IN_DISCARDS = "dot1dTpPortInDiscards" "1.3.6.1.2.1.17.4.4.1.5";
    }
    "Q-BRIDGE-MIB" {
        DOT1Q_VLAN_VERSION_NUMBER = "dot1qVlanVersionNumber" "1.3.6.1.2.1.17.7.1.1.1" scalar;
        DOT1Q_MAX_VLAN_ID = "dot1qMaxVlanId" "1.3.6.1.2.1.17.7.1.1.2" scalar;
        DOT1Q_MAX_SUPPORTED_VLANS = "dot1qMaxSupportedVlans" "1.3.6.1.2.1.17.7.1.1.3" scalar;
        DOT1Q_NUM_VLANS = "dot1qNumVlans" "1.3.6.1.2.1.17.7.1.1.4" scalar;
        DOT1Q_GVRP_STATUS = "dot1qGvrpStatus" "1.3.6.1.2.1.17.7.1.1.5" scalar;
        DOT1Q_FDB_TABLE = "dot1qFdbTable" "1.3.6.1.2.1.17.7.1.2.1";
        DOT1Q_FDB_DYNAMIC_COUNT = "dot1qFdbDynamicCount" "1.3.6.1.2.1.17.7.1.2.1.1.2";
        DOT1Q_TP_FDB_TABLE = "dot1qTpFdbTable" "1.3.6.1.2.1.17.7.1.2.2";
        DOT1Q_TP_FDB_PORT = "dot1qTpFdbPort" "1.3.6.1.2.1.17.7.1.2.2.1.2";
        DOT1Q_TP_FDB_STATUS = "dot1qTpFdbStatus" "1.3.6.1.2.1.17.7.1.2.2.1.3";
        DOT1Q_TP_GROUP_TABLE = "dot1qTpGroupTable" "1.3.6.1.2.1.17.7.1.2.3";
        DOT1Q_TP_GROUP_EGRESS_PORTS = "dot1qTpGroupEgressPorts" "1.3.6.1.2.1.17.7.1.2.3.1.2";
        DOT1Q_TP_GROUP_LEARNT = "dot1qTpGroupLearnt" "1.3.6.1.2.1.17.7.1.2.3.1.3";
        DOT1Q_FORWARD_ALL_TABLE = "dot1qForwardAllTable" "1.3.6.1.2.1.17.7.1.2.4";
        DOT1Q_FORWARD_ALL_PORTS = "dot1qForwardAllPorts" "1.3.6.1.2.1.17.7.1.2.4.1.1";
        DOT1Q_FORWARD_ALL_STATIC_PORTS = "dot1qForwardAllStaticPorts" "1.3.6.1.2.1.17.7.1.2.4.1.2";
        DOT1Q_FORWARD_ALL_FORBIDDEN_PORTS = "dot1qForwardAllForbiddenPorts" "1.3.6.1.2.1.17.7.1.2.4.1.3";
        DOT1Q_FORWARD_UNREGISTERED_TABLE = "dot1qForwardUnregisteredTable" "1.3.6.1.2.1.17.7.1.2.5";
        DOT1Q_FORWARD_UNREGISTERED_PORTS = "dot1qForwardUnregisteredPorts" "1.3.6.1.2.1.17.7.1.2.5.1.1";
        DOT1Q_FORWARD_UNREGISTERED_STATIC_PORTS = "dot1qForwardUnregisteredStaticPorts" "1.3.6.1.2.1.17.7.1.2.5.1.2";
        DOT1Q_FORWARD_UNREGISTERED_FORBIDDEN_PORTS = "dot1qForwardUnregisteredForbiddenPorts" "1.3.6.1.2.1.17.7.1.2.5.1.3";
        DOT1Q_VLAN_NUM_DELETES = "dot1qVlanNumDeletes" "1.3.6.1.2.1.17.7.1.4.1" scalar;
        DOT1Q_VLAN_CURRENT_TABLE = "dot1qVlanCurrentTable" "1.3.6.1.2.1.17.7.1.4.2";
        DOT1Q_VLAN_FDB_ID = "dot1qVlanFdbId" "1.3.6.1.2.1.17.7.1.4.2.1.3";
        DOT1Q_VLAN_CURRENT_EGRESS_PORTS = "dot1qVlanCurrentEgressPorts" "1.3.6.1.2.1.17.7.1.4.2.1.4";
        DOT1Q_VLAN_CURRENT_UNTAGGED_PORTS = "dot1qVlanCurrentUntaggedPorts" "1.3.6.1.2.1.17.7.1.4.2.1.5";
        DOT1Q_VLAN_STATUS = "dot1qVlanStatus" "1.3.6.1.2.1.17.7.1.4.2.1.6";
        DOT1Q_VLAN_CREATION_TIME = "dot1qVlanCreationTime" "1.3.6.1.2.1.17.7.1.4.2.1.7";
        DOT1Q_VLAN_STATIC_TABLE = "dot1qVlanStaticTable" "1.3.6.1.2.1.17.7.1.4.3";
        DOT1Q_VLAN_STATIC_NAME = "dot1qVlanStaticName" "1.3.6.1.2.1.17.7.1.4.3.1.1";
        DOT1Q_VLAN_STATIC_EGRESS_PORTS = "dot1qVlanStaticEgressPorts" "1.3.6.1.2.1.17.7.1.4.3.1.2";
        DOT1Q_VLAN_FORBIDDEN_EGRESS_PORTS = "dot1qVlanForbiddenEgressPorts" "1.3.6.1.2.1.17.7.1.4.3.1.3";
        DOT1Q_VLAN_STATIC_UNTAGGED_PORTS = "dot1qVlanStaticUntaggedPorts" "1.3.6.1.2.1.17.7.1.4.3.1.4";
        DOT1Q_VLAN_STATIC_ROW_STATUS = "dot1qVlanStaticRowStatus" "1.3.6.1.2.1.17.7.1.4.3.1.5";
        DOT1Q_NEXT_FREE_LOCAL_VLAN_INDEX = "dot1qNextFreeLocalVlanIndex" "1.3.6.1.2.1.17.7.1.4.4" scalar;
        DOT1Q_PORT_VLAN_TABLE = "dot1qPortVlanTable" "1.3.6.1.2.1.17.7.1.4.5";
        DOT1Q_PVID = "dot1qPvid" "1.3.6.1.2.1.17.7.1.4.5.1.1";
        DOT1Q_PORT_ACCEPTABLE_FRAME_TYPES = "dot1qPortAcceptableFrameTypes" "1.3.6.1.2.1.17.7.1.4.5.1.2";
        DOT1Q_PORT_INGRESS_FILTERING = "dot1qPortIngressFiltering" "1.3.6.1.2.1.17.7.1.4.5.1.3";
        DOT1Q_PORT_GVRP_STATUS = "dot1qPortGvrpStatus" "1.3.6.1.2.1.17.7.1.4.5.1.4";
        DOT1Q_PORT_GVRP_FAILED_REGISTRATIONS = "dot1qPortGvrpFailedRegistrations" "1.3.6.1.2.1.17.7.1.4.5.1.5";
        DOT1Q_PORT_GVRP_LAST_PDU_ORIGIN = "dot1qPortGvrpLastPduOrigin" "1.3.6.1.2.1.17.7.1.4.5.1.6";
        DOT1Q_PORT_RESTRICTED_VLAN_REGISTRATION = "dot1qPortRestrictedVlanRegistration" "1.3.6.1.2.1.17.7.1.4.5.1.7";
        DOT1Q_PORT_VLAN_STATISTICS_TABLE = "dot1qPortVlanStatisticsTable" "1.3.6.1.2.1.17.7.1.4.6";
        DOT1Q_TP_VLAN_PORT_IN_FRAMES = "dot1qTpVlanPortInFrames" "1.3.6.1.2.1.17.7.1.4.6.1.1";
        DOT1Q_TP_VLAN_PORT_OUT_FRAMES = "dot1qTpVlanPortOutFrames" "1.3.6.1.2.1.17.7.1.4.6.1.2";
        DOT1Q_TP_VLAN_PORT_IN_DISCARDS = "dot1qTpVlanPortInDiscards" "1.3.6.1.2.1.17.7.1.4.6.1.3";
        DOT1Q_TP_VLAN_PORT_IN_OVERFLOW_FRAMES = "dot1qTpVlanPortInOverflowFrames" "1.3.6.1.2.1.17.7.1.4.6.1.4";
        DOT1Q_TP_VLAN_PORT_OUT_OVERFLOW_FRAMES = "dot1qTpVlanPortOutOverflowFrames" "1.3.6.1.2.1.17.7.1.4.6.1.5";
        DOT1Q_TP_VLAN_PORT_IN_OVERFLOW_DISCARDS = "dot1qTpVlanPortInOverflowDiscards" "1.3.6.1.2.1.17.7.1.4.6.1.6";
        DOT1Q_PORT_VLAN_HC_STATISTICS_TABLE = "dot1qPortVlanHCStatisticsTable" "1.3.6.1.2.1.17.7.1.4.7";
        DOT1Q_TP_VLAN_PORT_HC_IN_FRAMES = "dot1qTpVlanPortHCInFrames" "1.3.6.1.2.1.17.7.1.4.7.1.1";
        DOT1Q_TP_VLAN_PORT_HC_OUT_FRAMES = "dot1qTpVlanPortHCOutFrames" "1.3.6.1.2.1.17.7.1.4.7.1.2";
        DOT1Q_TP_VLAN_PORT_HC_IN_DISCARDS = "dot1qTpVlanPortHCInDiscards" "1.3.6.1.2.1.17.7.1.4.7.1.3";
        DOT1Q_LEARNING_CONSTRAINTS_TABLE = "dot1qLearningConstraintsTable" "1.3.6.1.2.1.17.7.1.4.8";
        DOT1Q_CONSTRAINT_TYPE = "dot1qConstraintType" "1.3.6.1.2.1.17.7.1.4.8.1.3";
        DOT1Q_CONSTRAINT_STATUS = "dot1qConstraintStatus" "1.3.6.1.2.1.17.7.1.4.8.1.4";
        DOT1Q_CONSTRAINT_SET_DEFAULT = "dot1qConstraintSetDefault" "1.3.6.1.2.1.17.7.1.4.9" scalar;
        DOT1Q_CONSTRAINT_TYPE_DEFAULT = "dot1qConstraintTypeDefault" "1.3.6.1.2.1.17.7.1.4.10" scalar;
    }
    "LLDP-MIB" {
        LLDP_STATS_REM_TABLES_LAST_CHANGE_TIME = "lldpStatsRemTablesLastChangeTime" "1.0.8802.1.1.2.1.2.1" scalar;
        LLDP_STATS_REM_TABLES_INSERTS = "lldpStatsRemTablesInserts" "1.0.8802.1.1.2.1.2.2" scalar;
        LLDP_STATS_REM_TABLES_DELETES = "lldpStatsRemTablesDeletes" "1.0.8802.1.1.2.1.2.3" scalar;
        LLDP_STATS_REM_TABLES_DROPS = "lldpStatsRemTablesDrops" "1.0.8802.1.1.2.1.2.4" scalar;
        LLDP_STATS_REM_TABLES_AGEOUTS = "lldpStatsRemTablesAgeouts" "1.0.8802.1.1.2.1.2.5" scalar;
        LLDP_STATS_TX_PORT_TABLE = "lldpStatsTxPortTable" "1.0.8802.1.1.2.1.2.6";
        LLDP_STATS_TX_PORT_FRAMES_TOTAL = "lldpStatsTxPortFramesTotal" "1.0.8802.1.1.2.1.2.6.1.2";
        LLDP_STATS_RX_PORT_TABLE = "lldpStatsRxPortTable" "1.0.8802.1.1.2.1.2.7";
        LLDP_STATS_RX_PORT_FRAMES_DISCARDED_TOTAL = "lldpStatsRxPortFramesDiscardedTotal" "1.0.8802.1.1.2.1.2.7.1.2";
        LLDP_STATS_RX_PORT_FRAMES_ERRORS = "lldpStatsRxPortFramesErrors" "1.0.8802.1.1.2.1.2.7.1.3";
        LLDP_STATS_RX_PORT_FRAMES_TOTAL = "lldpStatsRxPortFramesTotal" "1.0.8802.1.1.2.1.2.7.1.4";
        LLDP_STATS_RX_PORT_TL_VS_DISCARDED_TOTAL = "lldpStatsRxPortTLVsDiscardedTotal" "1.0.8802.1.1.2.1.2.7.1.5";
        LLDP_STATS_RX_PORT_TL_VS_UNRECOGNIZED_TOTAL = "lldpStatsRxPortTLVsUnrecognizedTotal" "1.0.8802.1.1.2.1.2.7.1.6";
        LLDP_STATS_RX_PORT_AGEOUTS_TOTAL = "lldpStatsRxPortAgeoutsTotal" "1.0.8802.1.1.2.1.2.7.1.7";
        LLDP_LOC_CHASSIS_ID_SUBTYPE = "lldpLocChassisIdSubtype" "1.0.8802.1.1.2.1.3.1" scalar;
        LLDP_LOC_CHASSIS_ID = "lldpLocChassisId" "1.0.8802.1.1.2.1.3.2" scalar;
        LLDP_LOC_SYS_NAME = "lldpLocSysName" "1.0.8802.1.1.2.1.3.3" scalar;
        LLDP_LOC_SYS_DESC = "lldpLocSysDesc" "1.0.8802.1.1.2.1.3.4" scalar;
        LLDP_LOC_SYS_CAP_SUPPORTED = "lldpLocSysCapSupported" "1.0.8802.1.1.2.1.3.5" scalar;
        LLDP_LOC_SYS_CAP_ENABLED = "lldpLocSysCapEnabled" "1.0.8802.1.1.2.1.3.6" scalar;
        LLDP_LOC_PORT_TABLE = "lldpLocPortTable" "1.0.8802.1.1.2.1.3.7";
        LLDP_LOC_PORT_ID_SUBTYPE = "lldpLocPortIdSubtype" "1.0.8802.1.1.2.1.3.7.1.2";
        LLDP_LOC_PORT_ID = "lldpLocPortId" "1.0.8802.1.1.2.1.3.7.1.3";
        LLDP_LOC_PORT_DESC = "lldpLocPortDesc" "1.0.8802.1.1.2.1.3.7.1.4";
        LLDP_LOC_MAN_ADDR_TABLE = "lldpLocManAddrTable" "1.0.8802.1.1.2.1.3.8";
        LLDP_LOC_MAN_ADDR_LEN = "lldpLocManAddrLen" "1.0.8802.1.1.2.1.3.8.1.3";
        LLDP_LOC_MAN_ADDR_IF_SUBTYPE = "lldpLocManAddrIfSubtype" "1.0.8802.1.1.2.1.3.8.1.4";
        LLDP_LOC_MAN_ADDR_IF_ID = "lldpLocManAddrIfId" "1.0.8802.1.1.2.1.3.8.1.5";
        LLDP_LOC_MAN_ADDR_OID = "lldpLocManAddrOID" "1.0.8802.1.1.2.1.3.8.1.6";
        LLDP_REM_TABLE = "lldpRemTable" "1.0.8802.1.1.2.1.4.1";
        LLDP_REM_CHASSIS_ID_SUBTYPE = "lldpRemChassisIdSubtype" "1.0.8802.1.1.2.1.4.1.1.4";
        LLDP_REM_CHASSIS_ID = "lldpRemChassisId" "1.0.8802.1.1.2.1.4.1.1.5";
        LLDP_REM_PORT_ID_SUBTYPE = "lldpRemPortIdSubtype" "1.0.8802.1.1.2.1.4.1.1.6";
        LLDP_REM_PORT_ID = "lldpRemPortId" "1.0.8802.1.1.2.1.4.1.1.7";
        LLDP_REM_PORT_DESC = "lldpRemPortDesc" "1.0.8802.1.1.2.1.4.1.1.8";
        LLDP_REM_SYS_NAME = "lldpRemSysName" "1.0.8802.1.1.2.1.4.1.1.9";
        LLDP_REM_SYS_DESC = "lldpRemSysDesc" "1.0.8802.1.1.2.1.4.1.1.10";
        LLDP_REM_SYS_CAP_SUPPORTED = "lldpRemSysCapSupported" "1.0.8802.1.1.2.1.4.1.1.11";
        LLDP_REM_SYS_CAP_ENABLED = "lldpRemSysCapEnabled" "1.0.8802.1.1.2.1.4.1.1.12";
        LLDP_REM_MAN_ADDR_TABLE = "lldpRemManAddrTable" "1.0.8802.1.1.2.1.4.2";
        LLDP_REM_MAN_ADDR_IF_SUBTYPE = "lldpRemManAddrIfSubtype" "1.0.8802.1.1.2.1.4.2.1.3";
        LLDP_REM_MAN_ADDR_IF_ID = "lldpRemManAddrIfId" "1.0.8802.1.1.2.1.4.2.1.4";
        LLDP_REM_MAN_ADDR_OID = "lldpRemManAddrOID" "1.0.8802.1.1.2.1.4.2.1.5";
        LLDP_REM_UNKNOWN_TLV_TABLE = "lldpRemUnknownTLVTable" "1.0.8802.1.1.2.1.4.3";
        LLDP_REM_UNKNOWN_TLV_INFO = "lldpRemUnknownTLVInfo" "1.0.8802.1.1.2.1.4.3.1.2";
        LLDP_REM_ORG_DEF_INFO_TABLE = "lldpRemOrgDefInfoTable" "1.0.8802.1.1.2.1.4.4";
        LLDP_REM_ORG_DEF_INFO = "lldpRemOrgDefInfo" "1.0.8802.1.1.2.1.4.4.1.4";
    }
    "UCD-SNMP-MIB" {
        MEM_INDEX = "memIndex" "1.3.6.1.4.1.2021.4.1" scalar;
        MEM_ERROR_NAME = "memErrorName" "1.3.6.1.4.1.2021.4.2" scalar;
        MEM_TOTAL_SWAP = "memTotalSwap" "1.3.6.1.4.1.2021.4.3" scalar;
        MEM_AVAIL_SWAP = "memAvailSwap" "1.3.6.1.4.1.2021.4.4" scalar;
        MEM_TOTAL_REAL = "memTotalReal" "1.3.6.1.4.1.2021.4.5" scalar;
        MEM_AVAIL_REAL = "memAvailReal" "1.3.6.1.4.1.2021.4.6" scalar;
        MEM_TOTAL_SWAP_TXT = "memTotalSwapTXT" "1.3.6.1.4.1.2021.4.7" scalar;
        MEM_AVAIL_SWAP_TXT = "memAvailSwapTXT" "1.3.6.1.4.1.2021.4.8" scalar;
        MEM_TOTAL_REAL_TXT = "memTotalRealTXT" "1.3.6.1.4.1.2021.4.9" scalar;
        MEM_AVAIL_REAL_TXT = "memAvailRealTXT" "1.3.6.1.4.1.2021.4.10" scalar;
        MEM_TOTAL_FREE = "memTotalFree" "1.3.6.1.4.1.2021.4.11" scalar;
        MEM_MINIMUM_SWAP = "memMinimumSwap" "1.3.6.1.4.1.2021.4.12" scalar;
        MEM_SHARED = "memShared" "1.3.6.1.4.1.2021.4.13" scalar;
        MEM_BUFFER = "memBuffer" "1.3.6.1.4.1.2021.4.14" scalar;
        MEM_CACHED = "memCached" "1.3.6.1.4.1.2021.4.15" scalar;
        MEM_USED_SWAP_TXT = "memUsedSwapTXT" "1.3.6.1.4.1.2021.4.16" scalar;
        MEM_USED_REAL_TXT = "memUsedRealTXT" "1.3.6.1.4.1.2021.4.17" scalar;
        MEM_TOTAL_SWAP_X = "memTotalSwapX" "1.3.6.1.4.1.2021.4.18" scalar;
        MEM_AVAIL_SWAP_X = "memAvailSwapX" "1.3.6.1.4.1.2021.4.19" scalar;
        MEM_TOTAL_REAL_X = "memTotalRealX" "1.3.6.1.4.1.2021.4.20" scalar;
        MEM_AVAIL_REAL_X = "memAvailRealX" "1.3.6.1.4.1.2021.4.21" scalar;
        MEM_TOTAL_FREE_X = "memTotalFreeX" "1.3.6.1.4.1.2021.4.22" scalar;
        MEM_MINIMUM_SWAP_X = "memMinimumSwapX" "1.3.6.1.4.1.2021.4.23" scalar;
        MEM_SHARED_X = "memSharedX" "1.3.6.1.4.1.2021.4.24" scalar;
        MEM_BUFFER_X = "memBufferX" "1.3.6.1.4.1.2021.4.25" scalar;
        MEM_CACHED_X = "memCachedX" "1.3.6.1.4.1.2021.4.26" scalar;
        MEM_SYS_AVAIL = "memSysAvail" "1.3.6.1.4.1.2021.4.27" scalar;
        MEM_SWAP_ERROR = "memSwapError" "1.3.6.1.4.1.2021.4.100" scalar;
        MEM_SWAP_ERROR_MSG = "memSwapErrorMsg" "1.3.6.1.4.1.2021.4.101" scalar;
        DSK_TABLE = "dskTable" "1.3.6.1.4.1.2021.9";
        DSK_INDEX = "dskIndex" "1.3.6.1.4.1.2021.9.1.1";
        DSK_PATH = "dskPath" "1.3.6.1.4.1.2021.9.1.2";
        DSK_DEVICE = "dskDevice" "1.3.6.1.4.1.2021.9.1.3";
        DSK_MINIMUM = "dskMinimum" "1.3.6.1.4.1.2021.9.1.4";
        DSK_MIN_PERCENT = "dskMinPercent" "1.3.6.1.4.1.2021.9.1.5";
        DSK_TOTAL = "dskTotal" "1.3.6.1.4.1.2021.9.1.6";
        DSK_AVAIL = "dskAvail" "1.3.6.1.4.1.2021.9.1.7";
        DSK_USED = "dskUsed" "1.3.6.1.4.1.2021.9.1.8";
        DSK_PERCENT = "dskPercent" "1.3.6.1.4.1.2021.9.1.9";
        DSK_PERCENT_NODE = "dskPercentNode" "1.3.6.1.4.1.2021.9.1.10";
        DSK_TOTAL_LOW = "dskTotalLow" "1.3.6.1.4.1.2021.9.1.11";
        DSK_TOTAL_HIGH = "dskTotalHigh" "1.3.6.1.4.1.2021.9.1.12";
        DSK_AVAIL_LOW = "dskAvailLow" "1.3.6.1.4.1.2021.9.1.13";
        DSK_AVAIL_HIGH = "dskAvailHigh" "1.3.6.1.4.1.2021.9.1.14";
        DSK_USED_LOW = "dskUsedLow" "1.3.6.1.4.1.2021.9.1.15";
        DSK_USED_HIGH = "dskUsedHigh" "1.3.6.1.4.1.2021.9.1.16";
        DSK_ERROR_FLAG = "dskErrorFlag" "1.3.6.1.4.1.2021.9.1.100";
        DSK_ERROR_MSG = "dskErrorMsg" "1.3.6.1.4.1.2021.9.1.101";
        LA_TABLE = "laTable" "1.3.6.1.4.1.2021.10";
        LA_INDEX = "laIndex" "1.3.6.1.4.1.2021.10.1.1";
        LA_NAMES = "laNames" "1.3.6.1.4.1.2021.10.1.2";
        LA_LOAD = "laLoad" "1.3.6.1.4.1.2021.10.1.3";
        LA_CONFIG = "laConfig" "1.3.6.1.4.1.2021.10.1.4";
        LA_LOAD_INT = "laLoadInt" "1.3.6.1.4.1.2021.10.1.5";
        LA_LOAD_FLOAT = "laLoadFloat" "1.3.6.1.4.1.2021.10.1.6";
        LA_ERROR_FLAG = "laErrorFlag" "1.3.6.1.4.1.2021.10.1.100";
        LA_ERR_MESSAGE = "laErrMessage" "1.3.6.1.4.1.2021.10.1.101";
        SS_INDEX = "ssIndex" "1.3.6.1.4.1.2021.11.1" scalar;
        SS_ERROR_NAME = "ssErrorName" "1.3.6.1.4.1.2021.11.2" scalar;
        SS_SWAP_IN = "ssSwapIn" "1.3.6.1.4.1.2021.11.3" scalar;
        SS_SWAP_OUT = "ssSwapOut" "1.3.6.1.4.1.2021.11.4" scalar;
        SS_IO_SENT = "ssIOSent" "1.3.6.1.4.1.2021.11.5" scalar;
        SS_IO_RECEIVE = "ssIOReceive" "1.3.6.1.4.1.2021.11.6" scalar;
        SS_SYS_INTERRUPTS = "ssSysInterrupts" "1.3.6.1.4.1.2021.11.7" scalar;
        SS_SYS_CONTEXT = "ssSysContext" "1.3.6.1.4.1.2021.11.8" scalar;
        SS_CPU_USER = "ssCpuUser" "1.3.6.1.4.1.2021.11.9" scalar;
        SS_CPU_SYSTEM = "ssCpuSystem" "1.3.6.1.4.1.2021.11.10" scalar;
        SS_CPU_IDLE = "ssCpuIdle" "1.3.6.1.4.1.2021.11.11" scalar;
        SS_CPU_RAW_USER = "ssCpuRawUser" "1.3.6.1.4.1.2021.11.50" scalar;
        SS_CPU_RAW_NICE = "ssCpuRawNice" "1.3.6.1.4.1.2021.11.51" scalar;
        SS_CPU_RAW_SYSTEM = "ssCpuRawSystem" "1.3.6.1.4.1.2021.11.52" scalar;
        SS_CPU_RAW_IDLE = "ssCpuRawIdle" "1.3.6.1.4.1.2021.11.53" scalar;
        SS_CPU_RAW_WAIT = "ssCpuRawWait" "1.3.6.1.4.1.2021.11.54" scalar;
        SS_CPU_RAW_KERNEL = "ssCpuRawKernel" "1.3.6.1.4.1.2021.11.55" scalar;
        SS_CPU_RAW_INTERRUPT = "ssCpuRawInterrupt" "1.3.6.1.4.1.2021.11.56" scalar;
        SS_IO_RAW_SENT = "ssIORawSent" "1.3.6.1.4.1.2021.11.57" scalar;
        SS_IO_RAW_RECEIVED = "ssIORawReceived" "1.3.6.1.4.1.2021.11.58" scalar;
        SS_RAW_INTERRUPTS = "ssRawInterrupts" "1.3.6.1.4.1.2021.11.59" scalar;
        SS_RAW_CONTEXTS = "ssRawContexts" "1.3.6.1.4.1.2021.11.60" scalar;
        SS_CPU_RAW_SOFT_IRQ = "ssCpuRawSoftIRQ" "1.3.6.1.4.1.2021.11.61" scalar;
        SS_RAW_SWAP_IN = "ssRawSwapIn" "1.3.6.1.4.1.2021.11.62" scalar;
        SS_RAW_SWAP_OUT = "ssRawSwapOut" "1.3.6.1.4.1.2021.11.63" scalar;
        SS_CPU_RAW_STEAL = "ssCpuRawSteal" "1.3.6.1.4.1.2021.11.64" scalar;
        SS_CPU_RAW_GUEST = "ssCpuRawGuest" "1.3.6.1.4.1.2021.11.65" scalar;
        SS_CPU_RAW_GUEST_NICE = "ssCpuRawGuestNice" "1.3.6.1.4.1.2021.11.66" scalar;
        SS_CPU_NUM_CPUS = "ssCpuNumCpus" "1.3.6.1.4.1.2021.11.67" scalar;
    }
    "LM-SENSORS-MIB" {
        LM_TEMP_SENSORS_TABLE = "lmTempSensorsTable" "1.3.6.1.4.1.2021.13.16.2";
        LM_TEMP_SENSORS_INDEX = "lmTempSensorsIndex" "1.3.6.1.4.1.2021.13.16.2.1.1";
        LM_TEMP_SENSORS_DEVICE = "lmTempSensorsDevice" "1.3.6.1.4.1.2021.13.16.2.1.2";
        LM_TEMP_SENSORS_VALUE = "lmTempSensorsValue" "1.3.6.1.4.1.2021.13.16.2.1.3";
        LM_FAN_SENSORS_TABLE = "lmFanSensorsTable" "1.3.6.1.4.1.2021.13.16.3";
        LM_FAN_SENSORS_INDEX = "lmFanSensorsIndex" "1.3.6.1.4.1.2021.13.16.3.1.1";
        LM_FAN_SENSORS_DEVICE = "lmFanSensorsDevice" "1.3.6.1.4.1.2021.13.16.3.1.2";
        LM_FAN_SENSORS_VALUE = "lmFanSensorsValue" "1.3.6.1.4.1.2021.13.16.3.1.3";
        LM_VOLT_SENSORS_TABLE = "lmVoltSensorsTable" "1.3.6.1.4.1.2021.13.16.4";
        LM_VOLT_SENSORS_INDEX = "lmVoltSensorsIndex" "1.3.6.1.4.1.2021.13.16.4.1.1";
        LM_VOLT_SENSORS_DEVICE = "lmVoltSensorsDevice" "1.3.6.1.4.1.2021.13.16.4.1.2";
        LM_VOLT_SENSORS_VALUE = "lmVoltSensorsValue" "1.3.6.1.4.1.2021.13.16.4.1.3";
        LM_MISC_SENSORS_TABLE = "lmMiscSensorsTable" "1.3.6.1.4.1.2021.13.16.5";
        LM_MISC_SENSORS_INDEX = "lmMiscSensorsIndex" "1.3.6.1.4.1.2021.13.16.5.1.1";
        LM_MISC_SENSORS_DEVICE = "lmMiscSensorsDevice" "1.3.6.1.4.1.2021.13.16.5.1.2";
        LM_MISC_SENSORS_VALUE = "lmMiscSensorsValue" "1.3.6.1.4.1.2021.13.16.5.1.3";
    }
    "CISCO-PROCESS-MIB" {
        CPM_CPU_TOTAL_TABLE = "cpmCPUTotalTable" "1.3.6.1.4.1.9.9.109.1.1.1";
        CPM_CPU_TOTAL_PHYSICAL_INDEX = "cpmCPUTotalPhysicalIndex" "1.3.6.1.4.1.9.9.109.1.1.1.1.2";
        CPM_CPU_TOTAL5SEC = "cpmCPUTotal5sec" "1.3.6.1.4.1.9.9.109.1.1.1.1.3";
        CPM_CPU_TOTAL1MIN = "cpmCPUTotal1min" "1.3.6.1.4.1.9.9.109.1.1.1.1.4";
        CPM_CPU_TOTAL5MIN = "cpmCPUTotal5min" "1.3.6.1.4.1.9.9.109.1.1.1.1.5";
        CPM_CPU_TOTAL5SEC_REV = "cpmCPUTotal5secRev" "1.3.6.1.4.1.9.9.109.1.1.1.1.6";
        CPM_CPU_TOTAL1MIN_REV = "cpmCPUTotal1minRev" "1.3.6.1.4.1.9.9.109.1.1.1.1.7";
        CPM_CPU_TOTAL5MIN_REV = "cpmCPUTotal5minRev" "1.3.6.1.4.1.9.9.109.1.1.1.1.8";
        CPM_CPU_MON_INTERVAL = "cpmCPUMonInterval" "1.3.6.1.4.1.9.9.109.1.1.1.1.9";
        CPM_CPU_TOTAL_MON_INTERVAL_VALUE = "cpmCPUTotalMonIntervalValue" "1.3.6.1.4.1.9.9.109.1.1.1.1.10";
        CPM_CPU_INTERRUPT_MON_INTERVAL_VALUE = "cpmCPUInterruptMonIntervalValue" "1.3.6.1.4.1.9.9.109.1.1.1.1.11";
        CPM_CPU_MEMORY_USED = "cpmCPUMemoryUsed" "1.3.6.1.4.1.9.9.109.1.1.1.1.12";
        CPM_CPU_MEMORY_FREE = "cpmCPUMemoryFree" "1.3.6.1.4.1.9.9.109.1.1.1.1.13";
        CPM_CPU_MEMORY_KERNEL_RESERVED = "cpmCPUMemoryKernelReserved" "1.3.6.1.4.1.9.9.109.1.1.1.1.14";
        CPM_CPU_MEMORY_LOWEST = "cpmCPUMemoryLowest" "1.3.6.1.4.1.9.9.109.1.1.1.1.15";
        CPM_CPU_MEMORY_USED_OVRFLW = "cpmCPUMemoryUsedOvrflw" "1.3.6.1.4.1.9.9.109.1.1.1.1.16";
        CPM_CPU_MEMORY_HC_USED = "cpmCPUMemoryHCUsed" "1.3.6.1.4.1.9.9.109.1.1.1.1.17";
        CPM_CPU_MEMORY_FREE_OVRFLW = "cpmCPUMemoryFreeOvrflw" "1.3.6.1.4.1.9.9.109.1.1.1.1.18";
        CPM_CPU_MEMORY_HC_FREE = "cpmCPUMemoryHCFree" "1.3.6.1.4.1.9.9.109.1.1.1.1.19";
        CPM_CPU_MEMORY_KERNEL_RESERVED_OVRFLW = "cpmCPUMemoryKernelReservedOvrflw" "1.3.6.1.4.1.9.9.109.1.1.1.1.20";
        CPM_CPU_MEMORY_HC_KERNEL_RESERVED = "cpmCPUMemoryHCKernelReserved" "1.3.6.1.4.1.9.9.109.1.1.1.1.21";
        CPM_CPU_MEMORY_LOWEST_OVRFLW = "cpmCPUMemoryLowestOvrflw" "1.3.6.1.4.1.9.9.109.1.1.1.1.22";
        CPM_CPU_MEMORY_HC_LOWEST = "cpmCPUMemoryHCLowest" "1.3.6.1.4.1.9.9.109.1.1.1.1.23";
        CPM_CPU_LOAD_AVG1MIN = "cpmCPULoadAvg1min" "1.3.6.1.4.1.9.9.109.1.1.1.1.24";
        CPM_CPU_LOAD_AVG5MIN = "cpmCPULoadAvg5min" "1.3.6.1.4.1.9.9.109.1.1.1.1.25";
        CPM_CPU_LOAD_AVG15MIN = "cpmCPULoadAvg15min" "1.3.6.1.4.1.9.9.109.1.1.1.1.26";
        CPM_CPU_MEMORY_COMMITTED = "cpmCPUMemoryCommitted" "1.3.6.1.4.1.9.9.109.1.1.1.1.27";
        CPM_CPU_MEMORY_COMMITTED_OVRFLW = "cpmCPUMemoryCommittedOvrflw" "1.3.6.1.4.1.9.9.109.1.1.1.1.28";
        CPM_CPU_MEMORY_HC_COMMITTED = "cpmCPUMemoryHCCommitted" "1.3.6.1.4.1.9.9.109.1.1.1.1.29";
    }
    "OLD-CISCO-CPU-MIB" {
        CISCO_BUSY_PER = "busyPer" "1.3.6.1.4.1.9.2.1.56" scalar;
        CISCO_AVG_BUSY1 = "avgBusy1" "1.3.6.1.4.1.9.2.1.57" scalar;
        CISCO_AVG_BUSY5 = "avgBusy5" "1.3.6.1.4.1.9.2.1.58" scalar;
        CISCO_IDLE_COUNT = "idleCount" "1.3.6.1.4.1.9.2.1.59" scalar;
        CISCO_IDLE_WIRED = "idleWired" "1.3.6.1.4.1.9.2.1.60" scalar;
    }
    "CISCO-MEMORY-POOL-MIB" {
        CISCO_MEMORY_POOL_TABLE = "ciscoMemoryPoolTable" "1.3.6.1.4.1.9.9.48.1.1";
        CISCO_MEMORY_POOL_NAME = "ciscoMemoryPoolName" "1.3.6.1.4.1.9.9.48.1.1.1.2";
        CISCO_MEMORY_POOL_ALTERNATE = "ciscoMemoryPoolAlternate" "1.3.6.1.4.1.9.9.48.1.1.1.3";
        CISCO_MEMORY_POOL_VALID = "ciscoMemoryPoolValid" "1.3.6.1.4.1.9.9.48.1.1.1.4";
        CISCO_MEMORY_POOL_USED = "ciscoMemoryPoolUsed" "1.3.6.1.4.1.9.9.48.1.1.1.5";
        CISCO_MEMORY_POOL_FREE = "ciscoMemoryPoolFree" "1.3.6.1.4.1.9.9.48.1.1.1.6";
        CISCO_MEMORY_POOL_LARGEST_FREE = "ciscoMemoryPoolLargestFree" "1.3.6.1.4.1.9.9.48.1.1.1.7";
        CISCO_MEMORY_POOL_LOW_MEMORY_NOTIF_THRESHOLD = "ciscoMemoryPoolLowMemoryNotifThreshold" "1.3.6.1.4.1.9.9.48.1.1.1.8";
    }
    "CISCO-ENHANCED-MEMPOOL-MIB" {
        CEMP_MEM_POOL_TABLE = "cempMemPoolTable" "1.3.6.1.4.1.9.9.221.1.1.1";
        CEMP_MEM_POOL_TYPE = "cempMemPoolType" "1.3.6.1.4.1.9.9.221.1.1.1.1.2";
        CEMP_MEM_POOL_NAME = "cempMemPoolName" "1.3.6.1.4.1.9.9.221.1.1.1.1.3";
        CEMP_MEM_POOL_PLATFORM_MEMORY = "cempMemPoolPlatformMemory" "1.3.6.1.4.1.9.9.221.1.1.1.1.4";
        CEMP_MEM_POOL_ALTERNATE = "cempMemPoolAlternate" "1.3.6.1.4.1.9.9.221.1.1.1.1.5";
        CEMP_MEM_POOL_VALID = "cempMemPoolValid" "1.3.6.1.4.1.9.9.221.1.1.1.1.6";
        CEMP_MEM_POOL_USED = "cempMemPoolUsed" "1.3.6.1.4.1.9.9.221.1.1.1.1.7";
        CEMP_MEM_POOL_FREE = "cempMemPoolFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.8";
        CEMP_MEM_POOL_LARGEST_FREE = "cempMemPoolLargestFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.9";
        CEMP_MEM_POOL_LOWEST_FREE = "cempMemPoolLowestFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.10";
        CEMP_MEM_POOL_USED_LOW_WATER_MARK = "cempMemPoolUsedLowWaterMark" "1.3.6.1.4.1.9.9.221.1.1.1.1.11";
        CEMP_MEM_POOL_ALLOC_HIT = "cempMemPoolAllocHit" "1.3.6.1.4.1.9.9.221.1.1.1.1.12";
        CEMP_MEM_POOL_ALLOC_MISS = "cempMemPoolAllocMiss" "1.3.6.1.4.1.9.9.221.1.1.1.1.13";
        CEMP_MEM_POOL_FREE_HIT = "cempMemPoolFreeHit" "1.3.6.1.4.1.9.9.221.1.1.1.1.14";
        CEMP_MEM_POOL_FREE_MISS = "cempMemPoolFreeMiss" "1.3.6.1.4.1.9.9.221.1.1.1.1.15";
        CEMP_MEM_POOL_SHARED = "cempMemPoolShared" "1.3.6.1.4.1.9.9.221.1.1.1.1.16";
        CEMP_MEM_POOL_USED_OVRFLW = "cempMemPoolUsedOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.17";
        CEMP_MEM_POOL_HC_USED = "cempMemPoolHCUsed" "1.3.6.1.4.1.9.9.221.1.1.1.1.18";
        CEMP_MEM_POOL_FREE_OVRFLW = "cempMemPoolFreeOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.19";
        CEMP_MEM_POOL_HC_FREE = "cempMemPoolHCFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.20";
        CEMP_MEM_POOL_LARGEST_FREE_OVRFLW = "cempMemPoolLargestFreeOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.21";
        CEMP_MEM_POOL_HC_LARGEST_FREE = "cempMemPoolHCLargestFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.22";
        CEMP_MEM_POOL_LOWEST_FREE_OVRFLW = "cempMemPoolLowestFreeOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.23";
        CEMP_MEM_POOL_HC_LOWEST_FREE = "cempMemPoolHCLowestFree" "1.3.6.1.4.1.9.9.221.1.1.1.1.24";
        CEMP_MEM_POOL_USED_LOW_WATER_MARK_OVRFLW = "cempMemPoolUsedLowWaterMarkOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.25";
        CEMP_MEM_POOL_HC_USED_LOW_WATER_MARK = "cempMemPoolHCUsedLowWaterMark" "1.3.6.1.4.1.9.9.221.1.1.1.1.26";
        CEMP_MEM_POOL_SHARED_OVRFLW = "cempMemPoolSharedOvrflw" "1.3.6.1.4.1.9.9.221.1.1.1.1.27";
        CEMP_MEM_POOL_HC_SHARED = "cempMemPoolHCShared" "1.3.6.1.4.1.9.9.221.1.1.1.1.28";
    }
    "CISCO-ENVMON-MIB" {
        CISCO_ENV_MON_PRESENT = "ciscoEnvMonPresent" "1.3.6.1.4.1.9.9.13.1.1" scalar;
        CISCO_ENV_MON_VOLTAGE_STATUS_TABLE = "ciscoEnvMonVoltageStatusTable" "1.3.6.1.4.1.9.9.13.1.2";
        CISCO_ENV_MON_VOLTAGE_STATUS_DESCR = "ciscoEnvMonVoltageStatusDescr" "1.3.6.1.4.1.9.9.13.1.2.1.2";
        CISCO_ENV_MON_VOLTAGE_STATUS_VALUE = "ciscoEnvMonVoltageStatusValue" "1.3.6.1.4.1.9.9.13.1.2.1.3";
        CISCO_ENV_MON_VOLTAGE_THRESHOLD_LOW = "ciscoEnvMonVoltageThresholdLow" "1.3.6.1.4.1.9.9.13.1.2.1.4";
        CISCO_ENV_MON_VOLTAGE_THRESHOLD_HIGH = "ciscoEnvMonVoltageThresholdHigh" "1.3.6.1.4.1.9.9.13.1.2.1.5";
        CISCO_ENV_MON_VOLTAGE_LAST_SHUTDOWN = "ciscoEnvMonVoltageLastShutdown" "1.3.6.1.4.1.9.9.13.1.2.1.6";
        CISCO_ENV_MON_VOLTAGE_STATE = "ciscoEnvMonVoltageState" "1.3.6.1.4.1.9.9.13.1.2.1.7";
        CISCO_ENV_MON_TEMPERATURE_STATUS_TABLE = "ciscoEnvMonTemperatureStatusTable" "1.3.6.1.4.1.9.9.13.1.3";
        CISCO_ENV_MON_TEMPERATURE_STATUS_DESCR = "ciscoEnvMonTemperatureStatusDescr" "1.3.6.1.4.1.9.9.13.1.3.1.2";
        CISCO_ENV_MON_TEMPERATURE_STATUS_VALUE = "ciscoEnvMonTemperatureStatusValue" "1.3.6.1.4.1.9.9.13.1.3.1.3";
        CISCO_ENV_MON_TEMPERATURE_THRESHOLD = "ciscoEnvMonTemperatureThreshold" "1.3.6.1.4.1.9.9.13.1.3.1.4";
        CISCO_ENV_MON_TEMPERATURE_LAST_SHUTDOWN = "ciscoEnvMonTemperatureLastShutdown" "1.3.6.1.4.1.9.9.13.1.3.1.5";
        CISCO_ENV_MON_TEMPERATURE_STATE = "ciscoEnvMonTemperatureState" "1.3.6.1.4.1.9.9.13.1.3.1.6";
        CISCO_ENV_MON_TEMPERATURE_STATUS_VALUE_REV1 = "ciscoEnvMonTemperatureStatusValueRev1" "1.3.6.1.4.1.9.9.13.1.3.1.7";
        CISCO_ENV_MON_FAN_STATUS_TABLE = "ciscoEnvMonFanStatusTable" "1.3.6.1.4.1.9.9.13.1.4";
        CISCO_ENV_MON_FAN_STATUS_DESCR = "ciscoEnvMonFanStatusDescr" "1.3.6.1.4.1.9.9.13.1.4.1.2";
        CISCO_ENV_MON_FAN_STATE = "ciscoEnvMonFanState" "1.3.6.1.4.1.9.9.13.1.4.1.3";
        CISCO_ENV_MON_SUPPLY_STATUS_TABLE = "ciscoEnvMonSupplyStatusTable" "1.3.6.1.4.1.9.9.13.1.5";
        CISCO_ENV_MON_SUPPLY_STATUS_DESCR = "ciscoEnvMonSupplyStatusDescr" "1.3.6.1.4.1.9.9.13.1.5.1.2";
        CISCO_ENV_MON_SUPPLY_STATE = "ciscoEnvMonSupplyState" "1.3.6.1.4.1.9.9.13.1.5.1.3";
        CISCO_ENV_MON_SUPPLY_SOURCE = "ciscoEnvMonSupplySource" "1.3.6.1.4.1.9.9.13.1.5.1.4";
        CISCO_ENV_MON_ALARM_CONTACTS = "ciscoEnvMonAlarmContacts" "1.3.6.1.4.1.9.9.13.1.6" scalar;
    }
    "CISCO-ENTITY-SENSOR-MIB" {
        ENT_SENSOR_VALUE_TABLE = "entSensorValueTable" "1.3.6.1.4.1.9.9.91.1.1.1";
        ENT_SENSOR_TYPE = "entSensorType" "1.3.6.1.4.1.9.9.91.1.1.1.1.1";
        ENT_SENSOR_SCALE = "entSensorScale" "1.3.6.1.4.1.9.9.91.1.1.1.1.2";
        ENT_SENSOR_PRECISION = "entSensorPrecision" "1.3.6.1.4.1.9.9.91.1.1.1.1.3";
        ENT_SENSOR_VALUE = "entSensorValue" "1.3.6.1.4.1.9.9.91.1.1.1.1.4";
        ENT_SENSOR_STATUS = "entSensorStatus" "1.3.6.1.4.1.9.9.91.1.1.1.1.5";
        ENT_SENSOR_VALUE_TIME_STAMP = "entSensorValueTimeStamp" "1.3.6.1.4.1.9.9.91.1.1.1.1.6";
        ENT_SENSOR_VALUE_UPDATE_RATE = "entSensorValueUpdateRate" "1.3.6.1.4.1.9.9.91.1.1.1.1.7";
        ENT_SENSOR_MEASURED_ENTITY = "entSensorMeasuredEntity" "1.3.6.1.4.1.9.9.91.1.1.1.1.8";
    }
    "CISCO-CDP-MIB" {
        CDP_CACHE_TABLE = "cdpCacheTable" "1.3.6.1.4.1.9.9.23.1.2.1";
        CDP_CACHE_ADDRESS_TYPE = "cdpCacheAddressType" "1.3.6.1.4.1.9.9.23.1.2.1.1.3";
        CDP_CACHE_ADDRESS = "cdpCacheAddress" "1.3.6.1.4.1.9.9.23.1.2.1.1.4";
        CDP_CACHE_VERSION = "cdpCacheVersion" "1.3.6.1.4.1.9.9.23.1.2.1.1.5";
        CDP_CACHE_DEVICE_ID = "cdpCacheDeviceId" "1.3.6.1.4.1.9.9.23.1.2.1.1.6";
        CDP_CACHE_DEVICE_PORT = "cdpCacheDevicePort" "1.3.6.1.4.1.9.9.23.1.2.1.1.7";
        CDP_CACHE_PLATFORM = "cdpCachePlatform" "1.3.6.1.4.1.9.9.23.1.2.1.1.8";
        CDP_CACHE_CAPABILITIES = "cdpCacheCapabilities" "1.3.6.1.4.1.9.9.23.1.2.1.1.9";
        CDP_CACHE_VTP_MGMT_DOMAIN = "cdpCacheVTPMgmtDomain" "1.3.6.1.4.1.9.9.23.1.2.1.1.10";
        CDP_CACHE_NATIVE_VLAN = "cdpCacheNativeVLAN" "1.3.6.1.4.1.9.9.23.1.2.1.1.11";
        CDP_CACHE_DUPLEX = "cdpCacheDuplex" "1.3.6.1.4.1.9.9.23.1.2.1.1.12";
        CDP_CACHE_APPLIANCE_ID = "cdpCacheApplianceID" "1.3.6.1.4.1.9.9.23.1.2.1.1.13";
        CDP_CACHE_VLAN_ID = "cdpCacheVlanID" "1.3.6.1.4.1.9.9.23.1.2.1.1.14";
        CDP_CACHE_POWER_CONSUMPTION = "cdpCachePowerConsumption" "1.3.6.1.4.1.9.9.23.1.2.1.1.15";
        CDP_CACHE_MTU = "cdpCacheMTU" "1.3.6.1.4.1.9.9.23.1.2.1.1.16";
        CDP_CACHE_SYS_NAME = "cdpCacheSysName" "1.3.6.1.4.1.9.9.23.1.2.1.1.17";
        CDP_CACHE_SYS_OBJECT_ID = "cdpCacheSysObjectID" "1.3.6.1.4.1.9.9.23.1.2.1.1.18";
        CDP_CACHE_PRIMARY_MGMT_ADDR_TYPE = "cdpCachePrimaryMgmtAddrType" "1.3.6.1.4.1.9.9.23.1.2.1.1.19";
        CDP_CACHE_PRIMARY_MGMT_ADDR = "cdpCachePrimaryMgmtAddr" "1.3.6.1.4.1.9.9.23.1.2.1.1.20";
        CDP_CACHE_SECONDARY_MGMT_ADDR_TYPE = "cdpCacheSecondaryMgmtAddrType" "1.3.6.1.4.1.9.9.23.1.2.1.1.21";
        CDP_CACHE_SECONDARY_MGMT_ADDR = "cdpCacheSecondaryMgmtAddr" "1.3.6.1.4.1.9.9.23.1.2.1.1.22";
        CDP_CACHE_PHYS_LOCATION = "cdpCachePhysLocation" "1.3.6.1.4.1.9.9.23.1.2.1.1.23";
        CDP_CACHE_LAST_CHANGE = "cdpCacheLastChange" "1.3.6.1.4.1.9.9.23.1.2.1.1.24";
        CDP_GLOBAL_RUN = "cdpGlobalRun" "1.3.6.1.4.1.9.9.23.1.3.1" scalar;
        CDP_GLOBAL_MESSAGE_INTERVAL = "cdpGlobalMessageInterval" "1.3.6.1.4.1.9.9.23.1.3.2" scalar;
        CDP_GLOBAL_HOLD_TIME = "cdpGlobalHoldTime" "1.3.6.1.4.1.9.9.23.1.3.3" scalar;
        CDP_GLOBAL_DEVICE_ID = "cdpGlobalDeviceId" "1.3.6.1.4.1.9.9.23.1.3.4" scalar;
        CDP_GLOBAL_LAST_CHANGE = "cdpGlobalLastChange" "1.3.6.1.4.1.9.9.23.1.3.5" scalar;
        CDP_GLOBAL_DEVICE_ID_FORMAT_CPB = "cdpGlobalDeviceIdFormatCpb" "1.3.6.1.4.1.9.9.23.1.3.6" scalar;
        CDP_GLOBAL_DEVICE_ID_FORMAT = "cdpGlobalDeviceIdFormat" "1.3.6.1.4.1.9.9.23.1.3.7" scalar;
    }
    "MIKROTIK-MIB" {
        MTXR_HL_CORE_VOLTAGE = "mtxrHlCoreVoltage" "1.3.6.1.4.1.14988.1.1.3.1" scalar;
        MTXR_HL_THREE_DOT_THREE_VOLTAGE = "mtxrHlThreeDotThreeVoltage" "1.3.6.1.4.1.14988.1.1.3.2" scalar;
        MTXR_HL_FIVE_VOLTAGE = "mtxrHlFiveVoltage" "1.3.6.1.4.1.14988.1.1.3.3" scalar;
        MTXR_HL_TWELVE_VOLTAGE = "mtxrHlTwelveVoltage" "1.3.6.1.4.1.14988.1.1.3.4" scalar;
        MTXR_HL_SENSOR_TEMPERATURE = "mtxrHlSensorTemperature" "1.3.6.1.4.1.14988.1.1.3.5" scalar;
        MTXR_HL_CPU_TEMPERATURE = "mtxrHlCpuTemperature" "1.3.6.1.4.1.14988.1.1.3.6" scalar;
        MTXR_HL_BOARD_TEMPERATURE = "mtxrHlBoardTemperature" "1.3.6.1.4.1.14988.1.1.3.7" scalar;
        MTXR_HL_VOLTAGE = "mtxrHlVoltage" "1.3.6.1.4.1.14988.1.1.3.8" scalar;
        MTXR_HL_ACTIVE_FAN = "mtxrHlActiveFan" "1.3.6.1.4.1.14988.1.1.3.9" scalar;
        MTXR_HL_TEMPERATURE = "mtxrHlTemperature" "1.3.6.1.4.1.14988.1.1.3.10" scalar;
        MTXR_HL_PROCESSOR_TEMPERATURE = "mtxrHlProcessorTemperature" "1.3.6.1.4.1.14988.1.1.3.11" scalar;
        MTXR_HL_POWER = "mtxrHlPower" "1.3.6.1.4.1.14988.1.1.3.12" scalar;
        MTXR_HL_CURRENT = "mtxrHlCurrent" "1.3.6.1.4.1.14988.1.1.3.13" scalar;
        MTXR_HL_PROCESSOR_FREQUENCY = "mtxrHlProcessorFrequency" "1.3.6.1.4.1.14988.1.1.3.14" scalar;
        MTXR_HL_POWER_SUPPLY_STATE = "mtxrHlPowerSupplyState" "1.3.6.1.4.1.14988.1.1.3.15" scalar;
        MTXR_HL_BACKUP_POWER_SUPPLY_STATE = "mtxrHlBackupPowerSupplyState" "1.3.6.1.4.1.14988.1.1.3.16" scalar;
        MTXR_HL_FAN_SPEED1 = "mtxrHlFanSpeed1" "1.3.6.1.4.1.14988.1.1.3.17" scalar;
        MTXR_HL_FAN_SPEED2 = "mtxrHlFanSpeed2" "1.3.6.1.4.1.14988.1.1.3.18" scalar;
        MTXR_ALARM_SOCKET_STATUS = "mtxrAlarmSocketStatus" "1.3.6.1.4.1.14988.1.1.3.19" scalar;
        MTXR_GAUGE_TABLE = "mtxrGaugeTable" "1.3.6.1.4.1.14988.1.1.3.100";
        MTXR_GAUGE_NAME = "mtxrGaugeName" "1.3.6.1.4.1.14988.1.1.3.100.1.2";
        MTXR_GAUGE_VALUE = "mtxrGaugeValue" "1.3.6.1.4.1.14988.1.1.3.100.1.3";
        MTXR_GAUGE_UNIT = "mtxrGaugeUnit" "1.3.6.1.4.1.14988.1.1.3.100.1.4";
        MTXR_LIC_SOFTWARE_ID = "mtxrLicSoftwareId" "1.3.6.1.4.1.14988.1.1.4.1" scalar;
        MTXR_LIC_UPGR_UNTIL = "mtxrLicUpgrUntil" "1.3.6.1.4.1.14988.1.1.4.2" scalar;
        MTXR_LIC_LEVEL = "mtxrLicLevel" "1.3.6.1.4.1.14988.1.1.4.3" scalar;
        MTXR_LIC_VERSION = "mtxrLicVersion" "1.3.6.1.4.1.14988.1.1.4.4" scalar;
        MTXR_LIC_UPGRADABLE_TO = "mtxrLicUpgradableTo" "1.3.6.1.4.1.14988.1.1.4.5" scalar;
        MTXR_SYSTEM_REBOOT = "mtxrSystemReboot" "1.3.6.1.4.1.14988.1.1.7.1" scalar;
        MTXR_USB_POWER_RESET = "mtxrUSBPowerReset" "1.3.6.1.4.1.14988.1.1.7.2" scalar;
        MTXR_SERIAL_NUMBER = "mtxrSerialNumber" "1.3.6.1.4.1.14988.1.1.7.3" scalar;
        MTXR_FIRMWARE_VERSION = "mtxrFirmwareVersion" "1.3.6.1.4.1.14988.1.1.7.4" scalar;
        MTXR_NOTE = "mtxrNote" "1.3.6.1.4.1.14988.1.1.7.5" scalar;
        MTXR_BUILD_TIME = "mtxrBuildTime" "1.3.6.1.4.1.14988.1.1.7.6" scalar;
        MTXR_FIRMWARE_UPGRADE_VERSION = "mtxrFirmwareUpgradeVersion" "1.3.6.1.4.1.14988.1.1.7.7" scalar;
        MTXR_DISPLAY_NAME = "mtxrDisplayName" "1.3.6.1.4.1.14988.1.1.7.8" scalar;
        MTXR_BOARD_NAME = "mtxrBoardName" "1.3.6.1.4.1.14988.1.1.7.9" scalar;
    }
    "JUNIPER-MIB" {
        JNX_BOX_DESCR = "jnxBoxDescr" "1.3.6.1.4.1.2636.3.1.2" scalar;
        JNX_BOX_SERIAL_NO = "jnxBoxSerialNo" "1.3.6.1.4.1.2636.3.1.3" scalar;
        JNX_OPERATING_TABLE = "jnxOperatingTable" "1.3.6.1.4.1.2636.3.1.13";
        JNX_OPERATING_CONTENTS_INDEX = "jnxOperatingContentsIndex" "1.3.6.1.4.1.2636.3.1.13.1.1";
        JNX_OPERATING_L1_INDEX = "jnxOperatingL1Index" "1.3.6.1.4.1.2636.3.1.13.1.2";
        JNX_OPERATING_L2_INDEX = "jnxOperatingL2Index" "1.3.6.1.4.1.2636.3.1.13.1.3";
        JNX_OPERATING_L3_INDEX = "jnxOperatingL3Index" "1.3.6.1.4.1.2636.3.1.13.1.4";
        JNX_OPERATING_DESCR = "jnxOperatingDescr" "1.3.6.1.4.1.2636.3.1.13.1.5";
        JNX_OPERATING_STATE = "jnxOperatingState" "1.3.6.1.4.1.2636.3.1.13.1.6";
        JNX_OPERATING_TEMP = "jnxOperatingTemp" "1.3.6.1.4.1.2636.3.1.13.1.7";
        JNX_OPERATING_CPU = "jnxOperatingCPU" "1.3.6.1.4.1.2636.3.1.13.1.8";
        JNX_OPERATING_ISR = "jnxOperatingISR" "1.3.6.1.4.1.2636.3.1.13.1.9";
        JNX_OPERATING_DRAM_SIZE = "jnxOperatingDRAMSize" "1.3.6.1.4.1.2636.3.1.13.1.10";
        JNX_OPERATING_BUFFER = "jnxOperatingBuffer" "1.3.6.1.4.1.2636.3.1.13.1.11";
        JNX_OPERATING_HEAP = "jnxOperatingHeap" "1.3.6.1.4.1.2636.3.1.13.1.12";
        JNX_OPERATING_UP_TIME = "jnxOperatingUpTime" "1.3.6.1.4.1.2636.3.1.13.1.13";
        JNX_OPERATING_LAST_RESTART = "jnxOperatingLastRestart" "1.3.6.1.4.1.2636.3.1.13.1.14";
        JNX_OPERATING_MEMORY = "jnxOperatingMemory" "1.3.6.1.4.1.2636.3.1.13.1.15";
        JNX_OPERATING_STATE_ORDERED = "jnxOperatingStateOrdered" "1.3.6.1.4.1.2636.3.1.13.1.16";
        JNX_OPERATING_CHASSIS_ID = "jnxOperatingChassisId" "1.3.6.1.4.1.2636.3.1.13.1.17";
        JNX_OPERATING_CHASSIS_DESCR = "jnxOperatingChassisDescr" "1.3.6.1.4.1.2636.3.1.13.1.18";
        JNX_OPERATING_RESTART_TIME = "jnxOperatingRestartTime" "1.3.6.1.4.1.2636.3.1.13.1.19";
        JNX_OPERATING1_MIN_LOAD_AVG = "jnxOperating1MinLoadAvg" "1.3.6.1.4.1.2636.3.1.13.1.20";
        JNX_OPERATING5_MIN_LOAD_AVG = "jnxOperating5MinLoadAvg" "1.3.6.1.4.1.2636.3.1.13.1.21";
        JNX_OPERATING15_MIN_LOAD_AVG = "jnxOperating15MinLoadAvg" "1.3.6.1.4.1.2636.3.1.13.1.22";
        JNX_OPERATING1_MIN_AVG_CPU = "jnxOperating1MinAvgCPU" "1.3.6.1.4.1.2636.3.1.13.1.23";
        JNX_OPERATING5_MIN_AVG_CPU = "jnxOperating5MinAvgCPU" "1.3.6.1.4.1.2636.3.1.13.1.24";
        JNX_OPERATING15_MIN_AVG_CPU = "jnxOperating15MinAvgCPU" "1.3.6.1.4.1.2636.3.1.13.1.25";
        JNX_OPERATING_FRU_POWER = "jnxOperatingFRUPower" "1.3.6.1.4.1.2636.3.1.13.1.26";
        JNX_OPERATING_BUFFER_CP = "jnxOperatingBufferCP" "1.3.6.1.4.1.2636.3.1.13.1.27";
        JNX_OPERATING_MEMORY_CP = "jnxOperatingMemoryCP" "1.3.6.1.4.1.2636.3.1.13.1.28";
        JNX_OPERATING_BUFFER_EXT = "jnxOperatingBufferExt" "1.3.6.1.4.1.2636.3.1.13.1.29";
        JNX_OPERATING_TEMPERATURE = "jnxOperatingTemperature" "1.3.6.1.4.1.2636.3.1.13.1.30";
    }
    "ARISTA-ENTITY-SENSOR-MIB" {
        ARISTA_ENT_SENSOR_THRESHOLD_TABLE = "aristaEntSensorThresholdTable" "1.3.6.1.4.1.30065.3.12.1.1";
        ARISTA_ENT_SENSOR_THRESHOLD_LOW_WARNING = "aristaEntSensorThresholdLowWarning" "1.3.6.1.4.1.30065.3.12.1.1.1.1";
        ARISTA_ENT_SENSOR_THRESHOLD_LOW_CRITICAL = "aristaEntSensorThresholdLowCritical" "1.3.6.1.4.1.30065.3.12.1.1.1.2";
        ARISTA_ENT_SENSOR_THRESHOLD_HIGH_WARNING = "aristaEntSensorThresholdHighWarning" "1.3.6.1.4.1.30065.3.12.1.1.1.3";
        ARISTA_ENT_SENSOR_THRESHOLD_HIGH_CRITICAL = "aristaEntSensorThresholdHighCritical" "1.3.6.1.4.1.30065.3.12.1.1.1.4";
        ARISTA_ENT_SENSOR_STATUS_DESCR = "aristaEntSensorStatusDescr" "1.3.6.1.4.1.30065.3.12.1.1.1.5";
    }
    "HUAWEI-ENTITY-EXTENT-MIB" {
        HW_ENTITY_STATE_TABLE = "hwEntityStateTable" "1.3.6.1.4.1.2011.5.25.31.1.1.1";
        HW_ENTITY_ADMIN_STATUS = "hwEntityAdminStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.1";
        HW_ENTITY_OPER_STATUS = "hwEntityOperStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.2";
        HW_ENTITY_STANDBY_STATUS = "hwEntityStandbyStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.3";
        HW_ENTITY_ALARM_LIGHT = "hwEntityAlarmLight" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.4";
        HW_ENTITY_CPU_USAGE = "hwEntityCpuUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.5";
        HW_ENTITY_CPU_USAGE_THRESHOLD = "hwEntityCpuUsageThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.6";
        HW_ENTITY_MEM_USAGE = "hwEntityMemUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.7";
        HW_ENTITY_MEM_USAGE_THRESHOLD = "hwEntityMemUsageThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.8";
        HW_ENTITY_MEM_SIZE = "hwEntityMemSize" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.9";
        HW_ENTITY_UP_TIME = "hwEntityUpTime" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.10";
        HW_ENTITY_TEMPERATURE = "hwEntityTemperature" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.11";
        HW_ENTITY_TEMPERATURE_THRESHOLD = "hwEntityTemperatureThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.12";
        HW_ENTITY_VOLTAGE = "hwEntityVoltage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.13";
        HW_ENTITY_VOLTAGE_LOW_THRESHOLD = "hwEntityVoltageLowThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.14";
        HW_ENTITY_VOLTAGE_HIGH_THRESHOLD = "hwEntityVoltageHighThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.15";
        HW_ENTITY_TEMPERATURE_LOW_THRESHOLD = "hwEntityTemperatureLowThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.16";
        HW_ENTITY_OPTICAL_POWER = "hwEntityOpticalPower" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.17";
        HW_ENTITY_CURRENT = "hwEntityCurrent" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.18";
        HW_ENTITY_MEM_SIZE_MEGA = "hwEntityMemSizeMega" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.19";
        HW_ENTITY_PORT_TYPE = "hwEntityPortType" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.20";
        HW_ENTITY_DUPLEX = "hwEntityDuplex" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.21";
        HW_ENTITY_OPTICAL_POWER_RX = "hwEntityOpticalPowerRx" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.22";
        HW_ENTITY_CPU_USAGE_LOW_THRESHOLD = "hwEntityCpuUsageLowThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.23";
        HW_ENTITY_BOARD_POWER = "hwEntityBoardPower" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.24";
        HW_ENTITY_CPU_FREQUENCY = "hwEntityCpuFrequency" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.25";
        HW_ENTITY_SUPPORT_FLEX_CARD = "hwEntitySupportFlexCard" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.26";
        HW_ENTITY_BOARD_CLASS = "hwEntityBoardClass" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.27";
        HW_NSE_OPM_STATUS = "hwNseOpmStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.28";
        HW_ENTITY_CPU_MAX_USAGE = "hwEntityCpuMaxUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.29";
        HW_ENTITY_CPU_TYPE = "hwEntityCPUType" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.30";
        HW_ENTITY_MEMORY_TYPE = "hwEntityMemoryType" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.31";
        HW_ENTITY_FLASH_SIZE = "hwEntityFlashSize" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.32";
        HW_ENTITY_IF_UP_TIMES = "hwEntityIfUpTimes" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.33";
        HW_ENTITY_IF_DOWN_TIMES = "hwEntityIfDownTimes" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.34";
        HW_ENTITY_CPU_AVG_USAGE = "hwEntityCPUAvgUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.35";
        HW_ENTITY_MEMORY_AVG_USAGE = "hwEntityMemoryAvgUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.36";
        HW_ENTITY_MEM_USED = "hwEntityMemUsed" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.37";
        HW_ENTITY_TOTAL_FAN_NUM = "hwEntityTotalFanNum" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.38";
        HW_ENTITY_NOMAL_FAN_NUM = "hwEntityNomalFanNum" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.39";
        HW_ENTITY_TOTAL_PWR_NUM = "hwEntityTotalPwrNum" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.40";
        HW_ENTITY_NOMAL_PWR_NUM = "hwEntityNomalPwrNum" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.41";
        HW_ENTITY_FAULT_LIGHT = "hwEntityFaultLight" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.42";
        HW_ENTITY_BOARD_NAME = "hwEntityBoardName" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.43";
        HW_ENTITY_BOARD_DESCRIPTION = "hwEntityBoardDescription" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.44";
        HW_ENTITY5_MIN_CPU_USAGE = "hwEntity5MinCpuUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.45";
        HW_ENTITY_START_MODE = "hwEntityStartMode" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.46";
        HW_ENTITY_SPLIT_ATTRIBUTE = "hwEntitySplitAttribute" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.47";
        HW_ENTITY_FAULT_LIGHT_KEEP_TIME = "hwEntityFaultLightKeepTime" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.48";
        HW_ENTITY_PBUF_USAGE = "hwEntityPbufUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.49";
        HW_ENTITY_TM_USAGE = "hwEntityTMUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.50";
        HW_ENTITY_HDA1_USAGE = "hwEntityHda1Usage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.51";
        HW_ENTITY_HDA1_USAGE_THRESHOLD = "hwEntityHda1UsageThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.52";
        HW_ENTITY_HDA1_USAGE_RESUME_THRESHOLD = "hwEntityHda1UsageResumeThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.53";
        HW_ENTITY_MODEL_NAME = "hwEntityModelName" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.60";
        HW_ENTITY_ISSUE_NUMBER = "hwEntityIssueNumber" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.61";
        HW_ENTITY_DEVICE_STATUS = "hwEntityDeviceStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.62";
        HW_ENTITY_PIC_STATUS = "hwEntityPicStatus" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.63";
        HW_ENTITY_MPU_TYPE = "hwEntityMPUType" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.64";
        HW_ENTITY_MEM_SIZE_EXT = "hwEntityMemSizeExt" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.65";
        HW_ENTITY_TEMPERATURE_MINOR_THRESHOLD = "hwEntityTemperatureMinorThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.66";
        HW_ENTITY_VOLTAGE_FATAL_HIGH_THRESHOLD = "hwEntityVoltageFatalHighThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.67";
        HW_ENTITY_VOLTAGE_FATAL_LOW_THRESHOLD = "hwEntityVoltageFatalLowThreshold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.68";
        HW_ENTITY_MEM_CACHE_USAGE = "hwEntityMemCacheUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.69";
        HW_CPU_USAGE_TRAP_TYPE = "hwCpuUsageTrapType" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.70";
        HW_CPU_USAGE_TRAP_SLOT = "hwCpuUsageTrapSlot" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.71";
        HW_CPU_USAGE_TRAP_CPU = "hwCpuUsageTrapCpu" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.72";
        HW_CPU_USAGE_CURRENT_USAGE = "hwCpuUsageCurrentUsage" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.73";
        HW_CPU_USAGE_THREASHOLD = "hwCpuUsageThreashold" "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.74";
    }
    "STATISTICS-MIB" {
        HP_SWITCH_CPU_STAT = "hpSwitchCpuStat" "1.3.6.1.4.1.11.2.14.11.5.1.9.6.1" scalar;
    }
    "NETSWITCH-MIB" {
        HP_LOCAL_MEM_TABLE = "hpLocalMemTable" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1";
        HP_LOCAL_MEM_SLOT_INDEX = "hpLocalMemSlotIndex" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.1";
        HP_LOCAL_MEM_SLAB_CNT = "hpLocalMemSlabCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.2";
        HP_LOCAL_MEM_FREE_SEG_CNT = "hpLocalMemFreeSegCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.3";
        HP_LOCAL_MEM_ALLOC_SEG_CNT = "hpLocalMemAllocSegCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.4";
        HP_LOCAL_MEM_TOTAL_BYTES = "hpLocalMemTotalBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.5";
        HP_LOCAL_MEM_FREE_BYTES = "hpLocalMemFreeBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.6";
        HP_LOCAL_MEM_ALLOC_BYTES = "hpLocalMemAllocBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.1.1.1.7";
        HP_GLOBAL_MEM_TABLE = "hpGlobalMemTable" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1";
        HP_GLOBAL_MEM_SLOT_INDEX = "hpGlobalMemSlotIndex" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.1";
        HP_GLOBAL_MEM_SLAB_CNT = "hpGlobalMemSlabCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.2";
        HP_GLOBAL_MEM_FREE_SEG_CNT = "hpGlobalMemFreeSegCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.3";
        HP_GLOBAL_MEM_ALLOC_SEG_CNT = "hpGlobalMemAllocSegCnt" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.4";
        HP_GLOBAL_MEM_TOTAL_BYTES = "hpGlobalMemTotalBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.5";
        HP_GLOBAL_MEM_FREE_BYTES = "hpGlobalMemFreeBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.6";
        HP_GLOBAL_MEM_ALLOC_BYTES = "hpGlobalMemAllocBytes" "1.3.6.1.4.1.11.2.14.11.5.1.1.2.2.1.1.7";
    }
    "HP-ICF-CHASSIS" {
        HPICF_SENSOR_TABLE = "hpicfSensorTable" "1.3.6.1.4.1.11.2.14.11.1.2.6";
        HPICF_SENSOR_INDEX = "hpicfSensorIndex" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.1";
        HPICF_SENSOR_OBJECT_ID = "hpicfSensorObjectId" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.2";
        HPICF_SENSOR_NUMBER = "hpicfSensorNumber" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.3";
        HPICF_SENSOR_STATUS = "hpicfSensorStatus" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.4";
        HPICF_SENSOR_WARNINGS = "hpicfSensorWarnings" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.5";
        HPICF_SENSOR_FAILURES = "hpicfSensorFailures" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.6";
        HPICF_SENSOR_DESCR = "hpicfSensorDescr" "1.3.6.1.4.1.11.2.14.11.1.2.6.1.7";
        HP_SYSTEM_AIR_TEMP_TABLE = "hpSystemAirTempTable" "1.3.6.1.4.1.11.2.14.11.1.2.8.1";
        HP_SYSTEM_AIR_NAME = "hpSystemAirName" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.2";
        HP_SYSTEM_AIR_CURRENT_TEMP = "hpSystemAirCurrentTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.3";
        HP_SYSTEM_AIR_MAX_TEMP = "hpSystemAirMaxTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.4";
        HP_SYSTEM_AIR_MIN_TEMP = "hpSystemAirMinTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.5";
        HP_SYSTEM_AIR_OVER_TEMP = "hpSystemAirOverTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.6";
        HP_SYSTEM_AIR_THRESHOLD_TEMP = "hpSystemAirThresholdTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.7";
        HP_SYSTEM_AIR_AVG_TEMP = "hpSystemAirAvgTemp" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.8";
        HP_SYSTEM_AIR_ENT_PHYSICAL_INDEX = "hpSystemAirEntPhysicalIndex" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.9";
        HP_SYSTEM_AIR_AVG_TEMP1 = "hpSystemAirAvgTemp1" "1.3.6.1.4.1.11.2.14.11.1.2.8.1.1.10";
    }
    "HH3C-ENTITY-EXT-MIB" {
        HH3C_ENTITY_EXT_STATE_TABLE = "hh3cEntityExtStateTable" "1.3.6.1.4.1.25506.2.6.1.1.1";
        HH3C_ENTITY_EXT_ADMIN_STATUS = "hh3cEntityExtAdminStatus" "1.3.6.1.4.1.25506.2.6.1.1.1.1.2";
        HH3C_ENTITY_EXT_OPER_STATUS = "hh3cEntityExtOperStatus" "1.3.6.1.4.1.25506.2.6.1.1.1.1.3";
        HH3C_ENTITY_EXT_STANDBY_STATUS = "hh3cEntityExtStandbyStatus" "1.3.6.1.4.1.25506.2.6.1.1.1.1.4";
        HH3C_ENTITY_EXT_ALARM_LIGHT = "hh3cEntityExtAlarmLight" "1.3.6.1.4.1.25506.2.6.1.1.1.1.5";
        HH3C_ENTITY_EXT_CPU_USAGE = "hh3cEntityExtCpuUsage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.6";
        HH3C_ENTITY_EXT_CPU_USAGE_THRESHOLD = "hh3cEntityExtCpuUsageThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.7";
        HH3C_ENTITY_EXT_MEM_USAGE = "hh3cEntityExtMemUsage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.8";
        HH3C_ENTITY_EXT_MEM_USAGE_THRESHOLD = "hh3cEntityExtMemUsageThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.9";
        HH3C_ENTITY_EXT_MEM_SIZE = "hh3cEntityExtMemSize" "1.3.6.1.4.1.25506.2.6.1.1.1.1.10";
        HH3C_ENTITY_EXT_UP_TIME = "hh3cEntityExtUpTime" "1.3.6.1.4.1.25506.2.6.1.1.1.1.11";
        HH3C_ENTITY_EXT_TEMPERATURE = "hh3cEntityExtTemperature" "1.3.6.1.4.1.25506.2.6.1.1.1.1.12";
        HH3C_ENTITY_EXT_TEMPERATURE_THRESHOLD = "hh3cEntityExtTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.13";
        HH3C_ENTITY_EXT_VOLTAGE = "hh3cEntityExtVoltage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.14";
        HH3C_ENTITY_EXT_VOLTAGE_LOW_THRESHOLD = "hh3cEntityExtVoltageLowThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.15";
        HH3C_ENTITY_EXT_VOLTAGE_HIGH_THRESHOLD = "hh3cEntityExtVoltageHighThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.16";
        HH3C_ENTITY_EXT_CRITICAL_TEMPERATURE_THRESHOLD = "hh3cEntityExtCriticalTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.17";
        HH3C_ENTITY_EXT_MAC_ADDRESS = "hh3cEntityExtMacAddress" "1.3.6.1.4.1.25506.2.6.1.1.1.1.18";
        HH3C_ENTITY_EXT_ERROR_STATUS = "hh3cEntityExtErrorStatus" "1.3.6.1.4.1.25506.2.6.1.1.1.1.19";
        HH3C_ENTITY_EXT_CPU_MAX_USAGE = "hh3cEntityExtCpuMaxUsage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.20";
        HH3C_ENTITY_EXT_LOWER_TEMPERATURE_THRESHOLD = "hh3cEntityExtLowerTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.21";
        HH3C_ENTITY_EXT_SHUTDOWN_TEMPERATURE_THRESHOLD = "hh3cEntityExtShutdownTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.22";
        HH3C_ENTITY_EXT_PHY_MEM_SIZE = "hh3cEntityExtPhyMemSize" "1.3.6.1.4.1.25506.2.6.1.1.1.1.23";
        HH3C_ENTITY_EXT_PHY_CPU_FREQUENCY = "hh3cEntityExtPhyCpuFrequency" "1.3.6.1.4.1.25506.2.6.1.1.1.1.24";
        HH3C_ENTITY_EXT_FIRST_USED_DATE = "hh3cEntityExtFirstUsedDate" "1.3.6.1.4.1.25506.2.6.1.1.1.1.25";
        HH3C_ENTITY_EXT_CPU_AVG_USAGE = "hh3cEntityExtCpuAvgUsage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.26";
        HH3C_ENTITY_EXT_MEM_AVG_USAGE = "hh3cEntityExtMemAvgUsage" "1.3.6.1.4.1.25506.2.6.1.1.1.1.27";
        HH3C_ENTITY_EXT_MEM_TYPE = "hh3cEntityExtMemType" "1.3.6.1.4.1.25506.2.6.1.1.1.1.28";
        HH3C_ENTITY_EXT_CRITICAL_LOWER_TEMPERATURE_THRESHOLD = "hh3cEntityExtCriticalLowerTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.29";
        HH3C_ENTITY_EXT_SHUTDOWN_LOWER_TEMPERATURE_THRESHOLD = "hh3cEntityExtShutdownLowerTemperatureThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.30";
        HH3C_ENTITY_EXT_CPU_USAGE_RECOVER_THRESHOLD = "hh3cEntityExtCpuUsageRecoverThreshold" "1.3.6.1.4.1.25506.2.6.1.1.1.1.31";
        HH3C_ENTITY_EXT_MEM_SIZE_REV = "hh3cEntityExtMemSizeRev" "1.3.6.1.4.1.25506.2.6.1.1.1.1.32";
        HH3C_ENTITY_EXT_CPU_USAGE_IN1_MINUTE = "hh3cEntityExtCpuUsageIn1Minute" "1.3.6.1.4.1.25506.2.6.1.1.1.1.33";
        HH3C_ENTITY_EXT_CPU_USAGE_IN5_MINUTES = "hh3cEntityExtCpuUsageIn5Minutes" "1.3.6.1.4.1.25506.2.6.1.1.1.1.34";
    }
    "ARUBAWIRED-SYSTEMINFO-MIB" {
        ARUBA_WIRED_SYSTEM_INFO_TABLE = "arubaWiredSystemInfoTable" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1";
        ARUBA_WIRED_SYSTEM_INFO_CPU = "arubaWiredSystemInfoCpu" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.3";
        ARUBA_WIRED_SYSTEM_INFO_MEMORY = "arubaWiredSystemInfoMemory" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.4";
        ARUBA_WIRED_SYSTEM_INFO_STORAGE_NOS = "arubaWiredSystemInfoStorageNos" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.5";
        ARUBA_WIRED_SYSTEM_INFO_STORAGE_LOG = "arubaWiredSystemInfoStorageLog" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.6";
        ARUBA_WIRED_SYSTEM_INFO_STORAGE_COREDUMP = "arubaWiredSystemInfoStorageCoredump" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.7";
        ARUBA_WIRED_SYSTEM_INFO_STORAGE_SECURITY = "arubaWiredSystemInfoStorageSecurity" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.8";
        ARUBA_WIRED_SYSTEM_INFO_STORAGE_SELFTEST = "arubaWiredSystemInfoStorageSelftest" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.9";
        ARUBA_WIRED_SYSTEM_INFO_CPU_AVG_ONE_MIN = "arubaWiredSystemInfoCpuAvgOneMin" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.10";
        ARUBA_WIRED_SYSTEM_INFO_CPU_AVG_FIVE_MIN = "arubaWiredSystemInfoCpuAvgFiveMin" "1.3.6.1.4.1.47196.4.1.1.3.22.1.0.1.1.11";
    }
    "ARUBAWIRED-TEMPSENSOR-MIB" {
        ARUBA_WIRED_TEMP_SENSOR_TABLE = "arubaWiredTempSensorTable" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1";
        ARUBA_WIRED_TEMP_SENSOR_NAME = "arubaWiredTempSensorName" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1.1.5";
        ARUBA_WIRED_TEMP_SENSOR_STATE = "arubaWiredTempSensorState" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1.1.6";
        ARUBA_WIRED_TEMP_SENSOR_TEMPERATURE = "arubaWiredTempSensorTemperature" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1.1.7";
        ARUBA_WIRED_TEMP_SENSOR_MIN_TEMP = "arubaWiredTempSensorMinTemp" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1.1.8";
        ARUBA_WIRED_TEMP_SENSOR_MAX_TEMP = "arubaWiredTempSensorMaxTemp" "1.3.6.1.4.1.47196.4.1.1.3.11.3.1.1.9";
    }
    "ARUBAWIRED-FAN-MIB" {
        ARUBA_WIRED_FAN_TABLE = "arubaWiredFanTable" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1";
        ARUBA_WIRED_FAN_NAME = "arubaWiredFanName" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.4";
        ARUBA_WIRED_FAN_STATE = "arubaWiredFanState" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.5";
        ARUBA_WIRED_FAN_PRODUCT_NAME = "arubaWiredFanProductName" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.6";
        ARUBA_WIRED_FAN_SERIAL_NUMBER = "arubaWiredFanSerialNumber" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.7";
        ARUBA_WIRED_FAN_RPM = "arubaWiredFanRPM" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.8";
        ARUBA_WIRED_FAN_AIRFLOW_DIRECTION = "arubaWiredFanAirflowDirection" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.9";
        ARUBA_WIRED_FAN_STATE_ENUM = "arubaWiredFanStateEnum" "1.3.6.1.4.1.47196.4.1.1.3.11.5.1.1.10";
    }
    "ARUBAWIRED-POWERSUPPLY-MIB" {
        ARUBA_WIRED_POWER_SUPPLY_TABLE = "arubaWiredPowerSupplyTable" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1";
        ARUBA_WIRED_PSU_GROUP_INDEX = "arubaWiredPSUGroupIndex" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.1";
        ARUBA_WIRED_PSU_SLOT_INDEX = "arubaWiredPSUSlotIndex" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.2";
        ARUBA_WIRED_PSU_NAME = "arubaWiredPSUName" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.3";
        ARUBA_WIRED_PSU_STATE = "arubaWiredPSUState" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.4";
        ARUBA_WIRED_PSU_PRODUCT_NAME = "arubaWiredPSUProductName" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.5";
        ARUBA_WIRED_PSU_SERIAL_NUMBER = "arubaWiredPSUSerialNumber" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.6";
        ARUBA_WIRED_PSU_INSTANTANEOUS_POWER = "arubaWiredPSUInstantaneousPower" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.7";
        ARUBA_WIRED_PSU_MAXIMUM_POWER = "arubaWiredPSUMaximumPower" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.8";
        ARUBA_WIRED_PSU_NUMBER_FAILURES = "arubaWiredPSUNumberFailures" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.9";
        ARUBA_WIRED_PSU_AIRFLOW_DIRECTION = "arubaWiredPSUAirflowDirection" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.10";
        ARUBA_WIRED_PSU_STATE_ENUM = "arubaWiredPSUStateEnum" "1.3.6.1.4.1.47196.4.1.1.3.11.2.1.1.11";
    }
    "WLSX-SWITCH-MIB" {
        WLSX_SYS_X_PROCESSOR_TABLE = "wlsxSysXProcessorTable" "1.3.6.1.4.1.14823.2.2.1.1.1.9";
        WLSX_SYS_X_PROCESSOR_DESCR = "sysXProcessorDescr" "1.3.6.1.4.1.14823.2.2.1.1.1.9.1.2";
        WLSX_SYS_X_PROCESSOR_LOAD = "sysXProcessorLoad" "1.3.6.1.4.1.14823.2.2.1.1.1.9.1.3";
        WLSX_SYS_X_STORAGE_TABLE = "wlsxSysXStorageTable" "1.3.6.1.4.1.14823.2.2.1.1.1.10";
        WLSX_SYS_X_STORAGE_TYPE = "sysXStorageType" "1.3.6.1.4.1.14823.2.2.1.1.1.10.1.2";
        WLSX_SYS_X_STORAGE_SIZE = "sysXStorageSize" "1.3.6.1.4.1.14823.2.2.1.1.1.10.1.3";
        WLSX_SYS_X_STORAGE_USED = "sysXStorageUsed" "1.3.6.1.4.1.14823.2.2.1.1.1.10.1.4";
        WLSX_SYS_X_STORAGE_NAME = "sysXStorageName" "1.3.6.1.4.1.14823.2.2.1.1.1.10.1.5";
        WLSX_SYS_X_MEMORY_TABLE = "wlsxSysXMemoryTable" "1.3.6.1.4.1.14823.2.2.1.1.1.11";
        WLSX_SYS_X_MEMORY_SIZE = "sysXMemorySize" "1.3.6.1.4.1.14823.2.2.1.1.1.11.1.2";
        WLSX_SYS_X_MEMORY_USED = "sysXMemoryUsed" "1.3.6.1.4.1.14823.2.2.1.1.1.11.1.3";
        WLSX_SYS_X_MEMORY_FREE = "sysXMemoryFree" "1.3.6.1.4.1.14823.2.2.1.1.1.11.1.4";
    }
    "FORTINET-CORE-MIB" {
        FN_SYS_SERIAL = "fnSysSerial" "1.3.6.1.4.1.12356.100.1.1.1" scalar;
    }
    "FORTINET-FORTIGATE-MIB" {
        FG_SYS_VERSION = "fgSysVersion" "1.3.6.1.4.1.12356.101.4.1.1" scalar;
        FG_SYS_CPU_USAGE = "fgSysCpuUsage" "1.3.6.1.4.1.12356.101.4.1.3" scalar;
        FG_SYS_MEM_USAGE = "fgSysMemUsage" "1.3.6.1.4.1.12356.101.4.1.4" scalar;
        FG_SYS_MEM_CAPACITY = "fgSysMemCapacity" "1.3.6.1.4.1.12356.101.4.1.5" scalar;
        FG_SYS_DISK_USAGE = "fgSysDiskUsage" "1.3.6.1.4.1.12356.101.4.1.6" scalar;
        FG_SYS_DISK_CAPACITY = "fgSysDiskCapacity" "1.3.6.1.4.1.12356.101.4.1.7" scalar;
        FG_SYS_SES_COUNT = "fgSysSesCount" "1.3.6.1.4.1.12356.101.4.1.8" scalar;
        FG_SYS_LOW_MEM_USAGE = "fgSysLowMemUsage" "1.3.6.1.4.1.12356.101.4.1.9" scalar;
        FG_SYS_LOW_MEM_CAPACITY = "fgSysLowMemCapacity" "1.3.6.1.4.1.12356.101.4.1.10" scalar;
        FG_SYS_SES_RATE1 = "fgSysSesRate1" "1.3.6.1.4.1.12356.101.4.1.11" scalar;
        FG_SYS_SES_RATE10 = "fgSysSesRate10" "1.3.6.1.4.1.12356.101.4.1.12" scalar;
        FG_SYS_SES_RATE30 = "fgSysSesRate30" "1.3.6.1.4.1.12356.101.4.1.13" scalar;
        FG_SYS_SES_RATE60 = "fgSysSesRate60" "1.3.6.1.4.1.12356.101.4.1.14" scalar;
        FG_SYS_SES6_COUNT = "fgSysSes6Count" "1.3.6.1.4.1.12356.101.4.1.15" scalar;
        FG_SYS_SES6_RATE1 = "fgSysSes6Rate1" "1.3.6.1.4.1.12356.101.4.1.16" scalar;
        FG_SYS_SES6_RATE10 = "fgSysSes6Rate10" "1.3.6.1.4.1.12356.101.4.1.17" scalar;
        FG_SYS_SES6_RATE30 = "fgSysSes6Rate30" "1.3.6.1.4.1.12356.101.4.1.18" scalar;
        FG_SYS_SES6_RATE60 = "fgSysSes6Rate60" "1.3.6.1.4.1.12356.101.4.1.19" scalar;
        FG_SYS_UP_TIME = "fgSysUpTime" "1.3.6.1.4.1.12356.101.4.1.20" scalar;
        FG_SYS_NEED_LOG_PARTITION_SCAN = "fgSysNeedLogPartitionScan" "1.3.6.1.4.1.12356.101.4.1.21" scalar;
        FG_SYS_UP_TIME_DETAIL = "fgSysUpTimeDetail" "1.3.6.1.4.1.12356.101.4.1.22" scalar;
        FG_SYS_REBOOT_REASON = "fgSysRebootReason" "1.3.6.1.4.1.12356.101.4.1.23" scalar;
        FG_SYS_NPU_SES_COUNT = "fgSysNpuSesCount" "1.3.6.1.4.1.12356.101.4.1.24" scalar;
        FG_SYS_NPU_SES_RATE1 = "fgSysNpuSesRate1" "1.3.6.1.4.1.12356.101.4.1.25" scalar;
        FG_SYS_NPU_SES_RATE10 = "fgSysNpuSesRate10" "1.3.6.1.4.1.12356.101.4.1.26" scalar;
        FG_SYS_NPU_SES_RATE30 = "fgSysNpuSesRate30" "1.3.6.1.4.1.12356.101.4.1.27" scalar;
        FG_SYS_NPU_SES_RATE60 = "fgSysNpuSesRate60" "1.3.6.1.4.1.12356.101.4.1.28" scalar;
        FG_SYS_NPU_SES6_COUNT = "fgSysNpuSes6Count" "1.3.6.1.4.1.12356.101.4.1.29" scalar;
        FG_SYS_NPU_SES6_RATE1 = "fgSysNpuSes6Rate1" "1.3.6.1.4.1.12356.101.4.1.30" scalar;
        FG_SYS_NPU_SES6_RATE10 = "fgSysNpuSes6Rate10" "1.3.6.1.4.1.12356.101.4.1.31" scalar;
        FG_SYS_NPU_SES6_RATE30 = "fgSysNpuSes6Rate30" "1.3.6.1.4.1.12356.101.4.1.32" scalar;
        FG_SYS_NPU_SES6_RATE60 = "fgSysNpuSes6Rate60" "1.3.6.1.4.1.12356.101.4.1.33" scalar;
        FG_DATA_CPU_USAGE = "fgDataCpuUsage" "1.3.6.1.4.1.12356.101.4.1.34" scalar;
        FG_DATA_MEM_USAGE = "fgDataMemUsage" "1.3.6.1.4.1.12356.101.4.1.35" scalar;
        FG_SYS_FREE_MEM_USAGE = "fgSysFreeMemUsage" "1.3.6.1.4.1.12356.101.4.1.36" scalar;
        FG_SYS_FREEABLE_MEM_USAGE = "fgSysFreeableMemUsage" "1.3.6.1.4.1.12356.101.4.1.37" scalar;
        FG_DATA_SECURITY_LEVEL = "fgDataSecurityLevel" "1.3.6.1.4.1.12356.101.4.1.38" scalar;
        FG_HW_SENSOR_COUNT = "fgHwSensorCount" "1.3.6.1.4.1.12356.101.4.3.1" scalar;
        FG_HW_SENSOR_TABLE = "fgHwSensorTable" "1.3.6.1.4.1.12356.101.4.3.2";
        FG_HW_SENSOR_ENT_NAME = "fgHwSensorEntName" "1.3.6.1.4.1.12356.101.4.3.2.1.2";
        FG_HW_SENSOR_ENT_VALUE = "fgHwSensorEntValue" "1.3.6.1.4.1.12356.101.4.3.2.1.3";
        FG_HW_SENSOR_ENT_ALARM_STATUS = "fgHwSensorEntAlarmStatus" "1.3.6.1.4.1.12356.101.4.3.2.1.4";
        FG_PROCESSOR_COUNT = "fgProcessorCount" "1.3.6.1.4.1.12356.101.4.4.1" scalar;
        FG_PROCESSOR_TABLE = "fgProcessorTable" "1.3.6.1.4.1.12356.101.4.4.2";
        FG_PROCESSOR_USAGE = "fgProcessorUsage" "1.3.6.1.4.1.12356.101.4.4.2.1.2";
        FG_PROCESSOR_USAGE5SEC = "fgProcessorUsage5sec" "1.3.6.1.4.1.12356.101.4.4.2.1.3";
        FG_PROCESSOR_TYPE = "fgProcessorType" "1.3.6.1.4.1.12356.101.4.4.2.1.4";
        FG_PROCESSOR_CONTAINED_IN = "fgProcessorContainedIn" "1.3.6.1.4.1.12356.101.4.4.2.1.5";
        FG_PROCESSOR_PKT_RX_COUNT = "fgProcessorPktRxCount" "1.3.6.1.4.1.12356.101.4.4.2.1.6";
        FG_PROCESSOR_PKT_TX_COUNT = "fgProcessorPktTxCount" "1.3.6.1.4.1.12356.101.4.4.2.1.7";
        FG_PROCESSOR_PKT_DROPPED_COUNT = "fgProcessorPktDroppedCount" "1.3.6.1.4.1.12356.101.4.4.2.1.8";
        FG_PROCESSOR_USER_USAGE = "fgProcessorUserUsage" "1.3.6.1.4.1.12356.101.4.4.2.1.9";
        FG_PROCESSOR_SYS_USAGE = "fgProcessorSysUsage" "1.3.6.1.4.1.12356.101.4.4.2.1.10";
        FG_PROCESSOR_PKT_TX_DROPPED_COUNT = "fgProcessorPktTxDroppedCount" "1.3.6.1.4.1.12356.101.4.4.2.1.11";
    }
    "PAN-COMMON-MIB" {
        PAN_SYS_SW_VERSION = "panSysSwVersion" "1.3.6.1.4.1.25461.2.1.2.1.1" scalar;
        PAN_SYS_HW_VERSION = "panSysHwVersion" "1.3.6.1.4.1.25461.2.1.2.1.2" scalar;
        PAN_SYS_SERIAL_NUMBER = "panSysSerialNumber" "1.3.6.1.4.1.25461.2.1.2.1.3" scalar;
        PAN_SYS_TIME_ZONE_OFFSET = "panSysTimeZoneOffset" "1.3.6.1.4.1.25461.2.1.2.1.4" scalar;
        PAN_SYS_DAYLIGHT_SAVING = "panSysDaylightSaving" "1.3.6.1.4.1.25461.2.1.2.1.5" scalar;
        PAN_SYS_VPN_CLIENT_VERSION = "panSysVpnClientVersion" "1.3.6.1.4.1.25461.2.1.2.1.6" scalar;
        PAN_SYS_APP_VERSION = "panSysAppVersion" "1.3.6.1.4.1.25461.2.1.2.1.7" scalar;
        PAN_SYS_AV_VERSION = "panSysAvVersion" "1.3.6.1.4.1.25461.2.1.2.1.8" scalar;
        PAN_SYS_THREAT_VERSION = "panSysThreatVersion" "1.3.6.1.4.1.25461.2.1.2.1.9" scalar;
        PAN_SYS_URL_FILTERING_VERSION = "panSysUrlFilteringVersion" "1.3.6.1.4.1.25461.2.1.2.1.10" scalar;
        PAN_SYS_HA_STATE = "panSysHAState" "1.3.6.1.4.1.25461.2.1.2.1.11" scalar;
        PAN_SYS_HA_PEER_STATE = "panSysHAPeerState" "1.3.6.1.4.1.25461.2.1.2.1.12" scalar;
        PAN_SYS_HA_MODE = "panSysHAMode" "1.3.6.1.4.1.25461.2.1.2.1.13" scalar;
        PAN_SYS_URL_FILTERING_DATABASE = "panSysUrlFilteringDatabase" "1.3.6.1.4.1.25461.2.1.2.1.14" scalar;
        PAN_SYS_GLOBAL_PROTECT_CLIENT_VERSION = "panSysGlobalProtectClientVersion" "1.3.6.1.4.1.25461.2.1.2.1.15" scalar;
        PAN_SYS_OPSWAT_DATAFILE_VERSION = "panSysOpswatDatafileVersion" "1.3.6.1.4.1.25461.2.1.2.1.16" scalar;
        PAN_SYS_WILDFIRE_VERSION = "panSysWildfireVersion" "1.3.6.1.4.1.25461.2.1.2.1.17" scalar;
        PAN_SYS_WILDFIRE_PRIVATE_CLOUD_VERSION = "panSysWildfirePrivateCloudVersion" "1.3.6.1.4.1.25461.2.1.2.1.18" scalar;
        PAN_SYS_APP_RELEASE_DATE = "panSysAppReleaseDate" "1.3.6.1.4.1.25461.2.1.2.1.20" scalar;
        PAN_SYS_THREAT_RELEASE_DATE = "panSysThreatReleaseDate" "1.3.6.1.4.1.25461.2.1.2.1.21" scalar;
        PAN_SYS_AV_RELEASE_DATE = "panSysAvReleaseDate" "1.3.6.1.4.1.25461.2.1.2.1.22" scalar;
        PAN_SYS_WF_RELEASE_DATE = "panSysWfReleaseDate" "1.3.6.1.4.1.25461.2.1.2.1.23" scalar;
        PAN_SESSION_UTILIZATION = "panSessionUtilization" "1.3.6.1.4.1.25461.2.1.2.3.1" scalar;
        PAN_SESSION_MAX = "panSessionMax" "1.3.6.1.4.1.25461.2.1.2.3.2" scalar;
        PAN_SESSION_ACTIVE = "panSessionActive" "1.3.6.1.4.1.25461.2.1.2.3.3" scalar;
        PAN_SESSION_ACTIVE_TCP = "panSessionActiveTcp" "1.3.6.1.4.1.25461.2.1.2.3.4" scalar;
        PAN_SESSION_ACTIVE_UDP = "panSessionActiveUdp" "1.3.6.1.4.1.25461.2.1.2.3.5" scalar;
        PAN_SESSION_ACTIVE_ICMP = "panSessionActiveICMP" "1.3.6.1.4.1.25461.2.1.2.3.6" scalar;
        PAN_SESSION_ACTIVE_SSL_PROXY = "panSessionActiveSslProxy" "1.3.6.1.4.1.25461.2.1.2.3.7" scalar;
        PAN_SESSION_SSL_PROXY_UTILIZATION = "panSessionSslProxyUtilization" "1.3.6.1.4.1.25461.2.1.2.3.8" scalar;
        PAN_VSYS_TABLE = "panVsysTable" "1.3.6.1.4.1.25461.2.1.2.3.9";
        PAN_VSYS_ID = "panVsysId" "1.3.6.1.4.1.25461.2.1.2.3.9.1.1";
        PAN_VSYS_NAME = "panVsysName" "1.3.6.1.4.1.25461.2.1.2.3.9.1.2";
        PAN_VSYS_SESSION_UTILIZATION_PCT = "panVsysSessionUtilizationPct" "1.3.6.1.4.1.25461.2.1.2.3.9.1.3";
        PAN_VSYS_ACTIVE_SESSIONS = "panVsysActiveSessions" "1.3.6.1.4.1.25461.2.1.2.3.9.1.4";
        PAN_VSYS_MAX_SESSIONS = "panVsysMaxSessions" "1.3.6.1.4.1.25461.2.1.2.3.9.1.5";
        PAN_VSYS_ACTIVE_TCP_CPS = "panVsysActiveTcpCps" "1.3.6.1.4.1.25461.2.1.2.3.9.1.6";
        PAN_VSYS_ACTIVE_UDP_CPS = "panVsysActiveUdpCps" "1.3.6.1.4.1.25461.2.1.2.3.9.1.7";
        PAN_VSYS_ACTIVE_OTHER_IP_CPS = "panVsysActiveOtherIpCps" "1.3.6.1.4.1.25461.2.1.2.3.9.1.8";
        PAN_ZONE_TABLE = "panZoneTable" "1.3.6.1.4.1.25461.2.1.2.3.10";
        PAN_ZONE_NAME = "panZoneName" "1.3.6.1.4.1.25461.2.1.2.3.10.1.1";
        PAN_ZONE_ACTIVE_TCP_CPS = "panZoneActiveTcpCps" "1.3.6.1.4.1.25461.2.1.2.3.10.1.2";
        PAN_ZONE_ACTIVE_UDP_CPS = "panZoneActiveUdpCps" "1.3.6.1.4.1.25461.2.1.2.3.10.1.3";
        PAN_ZONE_ACTIVE_OTHER_IP_CPS = "panZoneActiveOtherIpCps" "1.3.6.1.4.1.25461.2.1.2.3.10.1.4";
        PAN_IF_TABLE = "panIfTable" "1.3.6.1.4.1.25461.2.1.2.3.11";
        PAN_IF_INDEX = "ifIndex" "1.3.6.1.4.1.25461.2.1.2.3.11.1.1";
        PAN_IF_DESCR = "ifDescr" "1.3.6.1.4.1.25461.2.1.2.3.11.1.2";
        PAN_IF_ACTIVE_TCP_CPS = "panIfActiveTcpCps" "1.3.6.1.4.1.25461.2.1.2.3.11.1.3";
        PAN_IF_ACTIVE_UDP_CPS = "panIfActiveUdpCps" "1.3.6.1.4.1.25461.2.1.2.3.11.1.4";
        PAN_IF_ACTIVE_OTHER_IP_CPS = "panIfActiveOtherIpCps" "1.3.6.1.4.1.25461.2.1.2.3.11.1.5";
    }
    "FROGFOOT-RESOURCES-MIB" {
        FROGFOOT_MEM_TOTAL = "memTotal" "1.3.6.1.4.1.10002.1.1.1.1.1" scalar;
        FROGFOOT_MEM_FREE = "memFree" "1.3.6.1.4.1.10002.1.1.1.1.2" scalar;
        FROGFOOT_MEM_BUFFER = "memBuffer" "1.3.6.1.4.1.10002.1.1.1.1.3" scalar;
        FROGFOOT_MEM_CACHE = "memCache" "1.3.6.1.4.1.10002.1.1.1.1.4" scalar;
        FROGFOOT_SWAP_TOTAL = "swapTotal" "1.3.6.1.4.1.10002.1.1.1.2.1" scalar;
        FROGFOOT_SWAP_FREE = "swapFree" "1.3.6.1.4.1.10002.1.1.1.2.2" scalar;
        FROGFOOT_DISK_NUMBER = "diskNumber" "1.3.6.1.4.1.10002.1.1.1.3.1" scalar;
        FROGFOOT_DISK_TABLE = "diskTable" "1.3.6.1.4.1.10002.1.1.1.3.2";
        FROGFOOT_DISK_DEV = "diskDev" "1.3.6.1.4.1.10002.1.1.1.3.2.1.2";
        FROGFOOT_DISK_DIR = "diskDir" "1.3.6.1.4.1.10002.1.1.1.3.2.1.3";
        FROGFOOT_DISK_FS_TYPE = "diskFSType" "1.3.6.1.4.1.10002.1.1.1.3.2.1.4";
        FROGFOOT_DISK_TOTAL = "diskTotal" "1.3.6.1.4.1.10002.1.1.1.3.2.1.5";
        FROGFOOT_DISK_FREE = "diskFree" "1.3.6.1.4.1.10002.1.1.1.3.2.1.6";
        FROGFOOT_LOAD_NUMBER = "loadNumber" "1.3.6.1.4.1.10002.1.1.1.4.1" scalar;
        FROGFOOT_LOAD_TABLE = "loadTable" "1.3.6.1.4.1.10002.1.1.1.4.2";
        FROGFOOT_LOAD_DESCR = "loadDescr" "1.3.6.1.4.1.10002.1.1.1.4.2.1.2";
        FROGFOOT_LOAD_VALUE = "loadValue" "1.3.6.1.4.1.10002.1.1.1.4.2.1.3";
    }
    "UBNT-UniFi-MIB" {
        UNIFI_AP_SYSTEM_IP = "unifiApSystemIp" "1.3.6.1.4.1.41112.1.6.3.1" scalar;
        UNIFI_AP_SYSTEM_ISOLATED = "unifiApSystemIsolated" "1.3.6.1.4.1.41112.1.6.3.2" scalar;
        UNIFI_AP_SYSTEM_MODEL = "unifiApSystemModel" "1.3.6.1.4.1.41112.1.6.3.3" scalar;
        UNIFI_AP_SYSTEM_UPLINK = "unifiApSystemUplink" "1.3.6.1.4.1.41112.1.6.3.4" scalar;
        UNIFI_AP_SYSTEM_UPTIME = "unifiApSystemUptime" "1.3.6.1.4.1.41112.1.6.3.5" scalar;
        UNIFI_AP_SYSTEM_VERSION = "unifiApSystemVersion" "1.3.6.1.4.1.41112.1.6.3.6" scalar;
    }
    "EdgeSwitch-SWITCHING-MIB" {
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_MEM_FREE = "agentSwitchCpuProcessMemFree" "1.3.6.1.4.1.4413.1.1.1.1.4.1" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_MEM_AVAILABLE = "agentSwitchCpuProcessMemAvailable" "1.3.6.1.4.1.4413.1.1.1.1.4.2" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_RISING_THRESHOLD = "agentSwitchCpuProcessRisingThreshold" "1.3.6.1.4.1.4413.1.1.1.1.4.3" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_RISING_THRESHOLD_INTERVAL = "agentSwitchCpuProcessRisingThresholdInterval" "1.3.6.1.4.1.4413.1.1.1.1.4.4" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_FALLING_THRESHOLD = "agentSwitchCpuProcessFallingThreshold" "1.3.6.1.4.1.4413.1.1.1.1.4.5" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_FALLING_THRESHOLD_INTERVAL = "agentSwitchCpuProcessFallingThresholdInterval" "1.3.6.1.4.1.4413.1.1.1.1.4.6" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_FREE_MEMORY_THRESHOLD = "agentSwitchCpuProcessFreeMemoryThreshold" "1.3.6.1.4.1.4413.1.1.1.1.4.7" scalar;
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_TABLE = "agentSwitchCpuProcessTable" "1.3.6.1.4.1.4413.1.1.1.1.4.8";
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_NAME = "agentSwitchCpuProcessName" "1.3.6.1.4.1.4413.1.1.1.1.4.8.1.2";
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_PERCENTAGE_UTILIZATION = "agentSwitchCpuProcessPercentageUtilization" "1.3.6.1.4.1.4413.1.1.1.1.4.8.1.3";
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_ID = "agentSwitchCpuProcessId" "1.3.6.1.4.1.4413.1.1.1.1.4.8.1.4";
        EDGESWITCH_AGENT_SWITCH_CPU_PROCESS_TOTAL_UTILIZATION = "agentSwitchCpuProcessTotalUtilization" "1.3.6.1.4.1.4413.1.1.1.1.4.9" scalar;
    }
    "EdgeSwitch-BOXSERVICES-PRIVATE-MIB" {
        EDGESWITCH_BOX_SERVICES_NORMAL_TEMP_RANGE_MIN = "boxServicesNormalTempRangeMin" "1.3.6.1.4.1.4413.1.1.43.1.1" scalar;
        EDGESWITCH_BOX_SERVICES_NORMAL_TEMP_RANGE_MAX = "boxServicesNormalTempRangeMax" "1.3.6.1.4.1.4413.1.1.43.1.2" scalar;
        EDGESWITCH_BOX_SERVICES_TEMPERATURE_TRAP_ENABLE = "boxServicesTemperatureTrapEnable" "1.3.6.1.4.1.4413.1.1.43.1.3" scalar;
        EDGESWITCH_BOX_SERVICES_PSM_STATE_TRAP_ENABLE = "boxServicesPSMStateTrapEnable" "1.3.6.1.4.1.4413.1.1.43.1.4" scalar;
        EDGESWITCH_BOX_SERVICES_FAN_STATE_TRAP_ENABLE = "boxServicesFanStateTrapEnable" "1.3.6.1.4.1.4413.1.1.43.1.5" scalar;
        EDGESWITCH_BOX_SERVICES_FANS_TABLE = "boxServicesFansTable" "1.3.6.1.4.1.4413.1.1.43.1.6";
        EDGESWITCH_BOX_SERVICES_FANS_INDEX = "boxServicesFansIndex" "1.3.6.1.4.1.4413.1.1.43.1.6.1.1";
        EDGESWITCH_BOX_SERVICES_FAN_ITEM_TYPE = "boxServicesFanItemType" "1.3.6.1.4.1.4413.1.1.43.1.6.1.2";
        EDGESWITCH_BOX_SERVICES_FAN_ITEM_STATE = "boxServicesFanItemState" "1.3.6.1.4.1.4413.1.1.43.1.6.1.3";
        EDGESWITCH_BOX_SERVICES_FAN_SPEED = "boxServicesFanSpeed" "1.3.6.1.4.1.4413.1.1.43.1.6.1.4";
        EDGESWITCH_BOX_SERVICES_FAN_DUTY_LEVEL = "boxServicesFanDutyLevel" "1.3.6.1.4.1.4413.1.1.43.1.6.1.5";
        EDGESWITCH_BOX_SERVICES_FAN_UNIT_INDEX = "boxServicesFanUnitIndex" "1.3.6.1.4.1.4413.1.1.43.1.6.1.6";
        EDGESWITCH_BOX_SERVICES_POW_SUPPLIES_TABLE = "boxServicesPowSuppliesTable" "1.3.6.1.4.1.4413.1.1.43.1.7";
        EDGESWITCH_BOX_SERVICES_POW_SUPPLY_INDEX = "boxServicesPowSupplyIndex" "1.3.6.1.4.1.4413.1.1.43.1.7.1.1";
        EDGESWITCH_BOX_SERVICES_POW_SUPPLY_ITEM_TYPE = "boxServicesPowSupplyItemType" "1.3.6.1.4.1.4413.1.1.43.1.7.1.2";
        EDGESWITCH_BOX_SERVICES_POW_SUPPLY_ITEM_STATE = "boxServicesPowSupplyItemState" "1.3.6.1.4.1.4413.1.1.43.1.7.1.3";
        EDGESWITCH_BOX_SERVICES_POWER_SUPP_UNIT_INDEX = "boxServicesPowerSuppUnitIndex" "1.3.6.1.4.1.4413.1.1.43.1.7.1.4";
        EDGESWITCH_BOX_SERVICES_TEMP_SENSORS_TABLE = "boxServicesTempSensorsTable" "1.3.6.1.4.1.4413.1.1.43.1.8";
        EDGESWITCH_BOX_SERVICES_UNIT_INDEX = "boxServicesUnitIndex" "1.3.6.1.4.1.4413.1.1.43.1.8.1.1";
        EDGESWITCH_BOX_SERVICES_TEMP_SENSOR_INDEX = "boxServicesTempSensorIndex" "1.3.6.1.4.1.4413.1.1.43.1.8.1.2";
        EDGESWITCH_BOX_SERVICES_TEMP_SENSOR_TYPE = "boxServicesTempSensorType" "1.3.6.1.4.1.4413.1.1.43.1.8.1.3";
        EDGESWITCH_BOX_SERVICES_TEMP_SENSOR_STATE = "boxServicesTempSensorState" "1.3.6.1.4.1.4413.1.1.43.1.8.1.4";
        EDGESWITCH_BOX_SERVICES_TEMP_SENSOR_TEMPERATURE = "boxServicesTempSensorTemperature" "1.3.6.1.4.1.4413.1.1.43.1.8.1.5";
        EDGESWITCH_BOX_SERVICES_TEMP_UNIT_TABLE = "boxServicesTempUnitTable" "1.3.6.1.4.1.4413.1.1.43.1.15";
        EDGESWITCH_BOX_SERVICES_TEMP_UNIT_INDEX = "boxServicesTempUnitIndex" "1.3.6.1.4.1.4413.1.1.43.1.15.1.1";
        EDGESWITCH_BOX_SERVICES_TEMP_UNIT_STATE = "boxServicesTempUnitState" "1.3.6.1.4.1.4413.1.1.43.1.15.1.2";
        EDGESWITCH_BOX_SERVICES_TEMP_UNIT_TEMPERATURE = "boxServicesTempUnitTemperature" "1.3.6.1.4.1.4413.1.1.43.1.15.1.3";
    }
}
