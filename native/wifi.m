#import <CoreWLAN/CoreWLAN.h>
#import <Foundation/Foundation.h>
#import <SystemConfiguration/SystemConfiguration.h>
#include <net/if.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define NM_WIFI_MAX_INTERFACES 16
#define NM_WIFI_SSID_CAPACITY 128
#define NM_WIFI_BSSID_CAPACITY 18
#define NM_WIFI_ERROR_CAPACITY 256

struct nm_wifi_interface {
    int64_t mode;
    uint32_t power_reported_on;
    int32_t link_state;
    uint32_t has_ssid;
    uint32_t ssid_length;
    uint32_t has_bssid;
    uint32_t bssid_length;
    char name[IFNAMSIZ];
    uint8_t ssid[NM_WIFI_SSID_CAPACITY];
    uint8_t bssid[NM_WIFI_BSSID_CAPACITY];
    char link_error[NM_WIFI_ERROR_CAPACITY];
};

struct nm_wifi_snapshot {
    uint32_t count;
    struct nm_wifi_interface interfaces[NM_WIFI_MAX_INTERFACES];
};

static int copy_optional_string(NSString *value, uint8_t *bytes, size_t capacity,
                                uint32_t *present, uint32_t *length,
                                const char *field, char *error, size_t error_size) {
    if (value == nil) return 0;
    NSData *encoded = [value dataUsingEncoding:NSUTF8StringEncoding allowLossyConversion:NO];
    if (encoded == nil || encoded.length > capacity) {
        snprintf(error, error_size, "%s cannot be represented in its UTF-8 buffer", field);
        return 1;
    }
    *present = 1;
    *length = (uint32_t)encoded.length;
    if (encoded.length != 0) memcpy(bytes, encoded.bytes, encoded.length);
    return 0;
}

static void read_link(SCDynamicStoreRef store, NSString *name,
                      struct nm_wifi_interface *out, const char *store_error) {
    out->link_state = -1;
    if (store == NULL) {
        snprintf(out->link_error, sizeof(out->link_error), "%s", store_error);
        return;
    }
    CFStringRef key = SCDynamicStoreKeyCreateNetworkInterfaceEntity(kCFAllocatorDefault,
        kSCDynamicStoreDomainState, (__bridge CFStringRef)name, kSCEntNetLink);
    if (key == NULL) {
        snprintf(out->link_error, sizeof(out->link_error),
            "cannot construct SystemConfiguration link key (%d)", SCError());
        return;
    }
    CFPropertyListRef value = SCDynamicStoreCopyValue(store, key);
    CFRelease(key);
    if (value == NULL) {
        snprintf(out->link_error, sizeof(out->link_error),
            "SystemConfiguration link state unavailable (%d)", SCError());
        return;
    }
    if (CFGetTypeID(value) != CFDictionaryGetTypeID()) {
        snprintf(out->link_error, sizeof(out->link_error),
            "SystemConfiguration link state is not a dictionary");
        CFRelease(value);
        return;
    }
    CFTypeRef active = CFDictionaryGetValue(value, kSCPropNetLinkActive);
    if (active == NULL || CFGetTypeID(active) != CFBooleanGetTypeID()) {
        snprintf(out->link_error, sizeof(out->link_error),
            "SystemConfiguration link Active value is missing or invalid");
    } else {
        out->link_state = CFBooleanGetValue(active) ? 1 : 0;
    }
    CFRelease(value);
}

int nm_read_wifi(struct nm_wifi_snapshot *out, char *error, size_t error_size) {
    @autoreleasepool {
        NSArray<CWInterface *> *interfaces = [[CWWiFiClient sharedWiFiClient] interfaces];
        if (interfaces == nil) {
            snprintf(error, error_size, "CoreWLAN interface query failed");
            return 1;
        }
        if (interfaces.count > NM_WIFI_MAX_INTERFACES) {
            snprintf(error, error_size, "CoreWLAN returned %lu interfaces; maximum is %d",
                (unsigned long)interfaces.count, NM_WIFI_MAX_INTERFACES);
            return 1;
        }
        memset(out, 0, sizeof(*out));
        SCDynamicStoreRef store = SCDynamicStoreCreate(kCFAllocatorDefault,
            CFSTR("network-monitor Wi-Fi observations"), NULL, NULL);
        char store_error[NM_WIFI_ERROR_CAPACITY] = {0};
        if (store == NULL) {
            snprintf(store_error, sizeof(store_error),
                "cannot open SystemConfiguration store (%d)", SCError());
        }
        int result = 0;
        for (CWInterface *interface in interfaces) {
            struct nm_wifi_interface *record = &out->interfaces[out->count];
            NSString *name = interface.interfaceName;
            NSData *name_bytes = [name dataUsingEncoding:NSUTF8StringEncoding
                                   allowLossyConversion:NO];
            if (name_bytes == nil || name_bytes.length == 0 ||
                name_bytes.length >= sizeof(record->name) ||
                memchr(name_bytes.bytes, 0, name_bytes.length) != NULL) {
                snprintf(error, error_size, "CoreWLAN interface name is missing or invalid");
                result = 1;
                break;
            }
            memcpy(record->name, name_bytes.bytes, name_bytes.length);
            record->power_reported_on = interface.powerOn ? 1 : 0;
            record->mode = (int64_t)interface.interfaceMode;
            read_link(store, name, record, store_error);
            if (copy_optional_string(interface.ssid, record->ssid, sizeof(record->ssid),
                    &record->has_ssid, &record->ssid_length, "SSID", error, error_size) ||
                copy_optional_string(interface.bssid, record->bssid, sizeof(record->bssid),
                    &record->has_bssid, &record->bssid_length, "BSSID", error, error_size)) {
                result = 1;
                break;
            }
            ++out->count;
        }
        if (store != NULL) CFRelease(store);
        return result;
    }
}
