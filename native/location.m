#import <CoreFoundation/CoreFoundation.h>
#import <CoreLocation/CoreLocation.h>
#import <Foundation/Foundation.h>

typedef int (*NMCallback)(void *context, int kind, double latitude,
                         double longitude, double accuracy, double source_time,
                         const char *message);

enum NMEventKind {
    NMPending = 0,
    NMDenied = 1,
    NMUnavailable = 2,
    NMFix = 3,
};

@interface NMLocationDelegate : NSObject <CLLocationManagerDelegate>
@property(nonatomic, strong) CLLocationManager *manager;
@property(nonatomic, assign) NMCallback callback;
@property(nonatomic, assign) void *context;
@property(nonatomic, assign) BOOL finished;
- (BOOL)emit:(int)kind
       latitude:(double)latitude
      longitude:(double)longitude
       accuracy:(double)accuracy
     sourceTime:(double)sourceTime
        message:(const char *)message;
- (void)updateAuthorization;
- (void)finish;
@end

@implementation NMLocationDelegate
- (void)finish {
    self.finished = YES;
    [self.manager stopUpdatingLocation];
    CFRunLoopStop(CFRunLoopGetMain());
}

- (BOOL)emit:(int)kind
       latitude:(double)latitude
      longitude:(double)longitude
       accuracy:(double)accuracy
     sourceTime:(double)sourceTime
        message:(const char *)message {
    if (self.finished) {
        return NO;
    }
    if (!self.callback(self.context, kind, latitude, longitude, accuracy,
                       sourceTime, message)) {
        [self finish];
        return NO;
    }
    return YES;
}

- (void)updateAuthorization {
    switch (self.manager.authorizationStatus) {
    case kCLAuthorizationStatusNotDetermined:
        [self emit:NMPending latitude:0 longitude:0 accuracy:0 sourceTime:0 message:NULL];
        break;
    case kCLAuthorizationStatusRestricted:
    case kCLAuthorizationStatusDenied:
        [self.manager stopUpdatingLocation];
        [self emit:NMDenied latitude:0 longitude:0 accuracy:0 sourceTime:0 message:NULL];
        break;
    case kCLAuthorizationStatusAuthorizedAlways:
        if (!self.finished) {
            [self.manager startUpdatingLocation];
        }
        break;
    default:
        [self emit:NMUnavailable latitude:0 longitude:0 accuracy:0 sourceTime:0
            message:"CoreLocation returned an unknown authorization status"];
        break;
    }
}

- (void)locationManagerDidChangeAuthorization:(CLLocationManager *)manager {
    (void)manager;
    [self updateAuthorization];
}

- (void)locationManager:(CLLocationManager *)manager
    didUpdateLocations:(NSArray<CLLocation *> *)locations {
    (void)manager;
    for (CLLocation *location in locations) {
        if (![self emit:NMFix
              latitude:location.coordinate.latitude
             longitude:location.coordinate.longitude
              accuracy:location.horizontalAccuracy
            sourceTime:location.timestamp.timeIntervalSince1970
               message:NULL]) {
            break;
        }
    }
}

- (void)locationManager:(CLLocationManager *)manager
      didFailWithError:(NSError *)error {
    (void)manager;
    if ([error.domain isEqualToString:kCLErrorDomain] && error.code == kCLErrorDenied) {
        [self emit:NMDenied latitude:0 longitude:0 accuracy:0 sourceTime:0 message:NULL];
    } else {
        NSString *message = [NSString stringWithFormat:@"%@ (%ld): %@",
            error.domain, (long)error.code, error.localizedDescription];
        [self emit:NMUnavailable latitude:0 longitude:0 accuracy:0 sourceTime:0
            message:message.UTF8String];
    }
}
@end

static void NMStop(CFFileDescriptorRef descriptor, CFOptionFlags flags, void *context) {
    (void)descriptor;
    (void)flags;
    NMLocationDelegate *delegate = (__bridge NMLocationDelegate *)context;
    [delegate finish];
}

int nm_location_run(NMCallback callback, void *context, int stop_fd) {
    if (![NSThread isMainThread]) {
        return 1;
    }
    @autoreleasepool {
        NMLocationDelegate *delegate = [NMLocationDelegate new];
        delegate.callback = callback;
        delegate.context = context;
        delegate.manager = [CLLocationManager new];
        delegate.manager.delegate = delegate;
        delegate.manager.desiredAccuracy = kCLLocationAccuracyKilometer;
        delegate.manager.distanceFilter = 100;

        CFFileDescriptorContext stop_context = {0, (__bridge void *)delegate, NULL, NULL, NULL};
        CFFileDescriptorRef descriptor = CFFileDescriptorCreate(kCFAllocatorDefault,
            stop_fd, false, NMStop, &stop_context);
        if (descriptor == NULL) {
            delegate.manager.delegate = nil;
            return 2;
        }
        CFRunLoopSourceRef source = CFFileDescriptorCreateRunLoopSource(kCFAllocatorDefault,
            descriptor, 0);
        if (source == NULL) {
            CFFileDescriptorInvalidate(descriptor);
            CFRelease(descriptor);
            delegate.manager.delegate = nil;
            return 2;
        }
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
        CFFileDescriptorEnableCallBacks(descriptor, kCFFileDescriptorReadCallBack);

        [delegate emit:NMPending latitude:0 longitude:0 accuracy:0 sourceTime:0 message:NULL];
        if (!delegate.finished && [CLLocationManager locationServicesEnabled]) {
            [delegate.manager requestWhenInUseAuthorization];
            [delegate updateAuthorization];
        } else if (!delegate.finished) {
            [delegate emit:NMUnavailable latitude:0 longitude:0 accuracy:0 sourceTime:0
                message:"macOS Location Services are disabled"];
        }
        while (!delegate.finished) {
            // Return after a source fires to release transient Foundation objects
            // without adding a polling timer to an otherwise idle run loop.
            @autoreleasepool {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0e9, true);
            }
        }

        // Rust owns the callback context until this call returns, so disconnect first.
        [delegate.manager stopUpdatingLocation];
        delegate.manager.delegate = nil;
        CFRunLoopRemoveSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
        CFRunLoopSourceInvalidate(source);
        CFFileDescriptorInvalidate(descriptor);
        CFRelease(source);
        CFRelease(descriptor);
    }
    return 0;
}
