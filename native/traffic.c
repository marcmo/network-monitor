#include <SystemConfiguration/SystemConfiguration.h>
#include <errno.h>
#include <net/if.h>
#include <net/if_mib.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/sysctl.h>

struct nm_traffic_counters {
    uint64_t received_bytes;
    uint64_t sent_bytes;
    uint64_t change_id;
    uint32_t index;
    char name[IFNAMSIZ];
};

static int primary_interface(char name[IFNAMSIZ]) {
    CFStringRef keys[] = {CFSTR("State:/Network/Global/IPv4"),
                         CFSTR("State:/Network/Global/IPv6")};
    for (unsigned int i = 0; i < sizeof(keys) / sizeof(keys[0]); ++i) {
        CFPropertyListRef value = SCDynamicStoreCopyValue(NULL, keys[i]);
        if (!value) continue;
        int found = 0;
        if (CFGetTypeID(value) == CFDictionaryGetTypeID()) {
            CFTypeRef interface = CFDictionaryGetValue(value, CFSTR("PrimaryInterface"));
            found = interface && CFGetTypeID(interface) == CFStringGetTypeID() &&
                CFStringGetCString(interface, name, IFNAMSIZ, kCFStringEncodingUTF8);
        }
        CFRelease(value);
        if (found) return 1;
    }
    return 0;
}

int nm_read_traffic(struct nm_traffic_counters *out, char *error, size_t error_size) {
    char name[IFNAMSIZ] = {0};
    if (!primary_interface(name)) {
        snprintf(error, error_size, "no primary IPv4 or IPv6 interface");
        return 1;
    }
    unsigned int index = if_nametoindex(name);
    if (!index) {
        snprintf(error, error_size, "primary interface disappeared");
        return 1;
    }
    int mib[] = {CTL_NET, PF_LINK, NETLINK_GENERIC, IFMIB_IFDATA, (int)index, IFDATA_GENERAL};
    struct ifmibdata data = {0};
    size_t size = sizeof(data);
    if (sysctl(mib, 6, &data, &size, NULL, 0) != 0) {
        snprintf(error, error_size, "interface counter query failed: %s", strerror(errno));
        return 1;
    }
    char after[IFNAMSIZ] = {0};
    if (size != sizeof(data) || !(data.ifmd_flags & IFF_UP) ||
        strncmp(name, data.ifmd_name, IFNAMSIZ) != 0 ||
        !primary_interface(after) || strcmp(name, after) != 0 || if_nametoindex(after) != index) {
        snprintf(error, error_size, "primary interface changed or is down");
        return 1;
    }
    out->received_bytes = data.ifmd_data.ifi_ibytes;
    out->sent_bytes = data.ifmd_data.ifi_obytes;
    // Reused interface indices and administrative changes must break rate continuity.
    out->change_id = ((uint64_t)(uint32_t)data.ifmd_data.ifi_lastchange.tv_sec << 32) |
                    (uint32_t)data.ifmd_data.ifi_lastchange.tv_usec;
    out->index = index;
    memcpy(out->name, name, IFNAMSIZ);
    return 0;
}
