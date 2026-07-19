#include <CoreFoundation/CoreFoundation.h>
#include <NetFS/NetFS.h>
#include <dispatch/dispatch.h>
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>

typedef void (*classick_netfs_completion)(void *context, int status,
                                          const char *mountpoint);

static char *classick_copy_mountpoint(CFArrayRef mountpoints) {
  if (mountpoints == NULL) {
    return NULL;
  }

  CFIndex count = CFArrayGetCount(mountpoints);
  for (CFIndex index = 0; index < count; index++) {
    CFTypeRef candidate = CFArrayGetValueAtIndex(mountpoints, index);
    if (candidate == NULL || CFGetTypeID(candidate) != CFStringGetTypeID()) {
      continue;
    }

    CFStringRef path = (CFStringRef)candidate;
    CFIndex capacity = CFStringGetMaximumSizeForEncoding(
                           CFStringGetLength(path), kCFStringEncodingUTF8) +
                       1;
    char *buffer = malloc((size_t)capacity);
    if (buffer != NULL &&
        CFStringGetCString(path, buffer, capacity, kCFStringEncodingUTF8)) {
      return buffer;
    }
    free(buffer);
  }
  return NULL;
}

int classick_netfs_mount_async(const char *url_utf8, int allow_ui,
                               classick_netfs_completion completion,
                               void *context) {
  if (url_utf8 == NULL || completion == NULL) {
    return EINVAL;
  }

  CFURLRef url = CFURLCreateWithBytes(
      kCFAllocatorDefault, (const UInt8 *)url_utf8, (CFIndex)strlen(url_utf8),
      kCFStringEncodingUTF8, NULL);
  CFMutableDictionaryRef open_options = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 0, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  CFMutableDictionaryRef mount_options = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 0, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  if (url == NULL || open_options == NULL || mount_options == NULL) {
    if (url != NULL)
      CFRelease(url);
    if (open_options != NULL)
      CFRelease(open_options);
    if (mount_options != NULL)
      CFRelease(mount_options);
    return ENOMEM;
  }

  CFDictionarySetValue(open_options, kNAUIOptionKey,
                       allow_ui ? kNAUIOptionAllowUI : kNAUIOptionNoUI);

  int mount_flags = MNT_RDONLY;
  CFNumberRef flags =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberIntType, &mount_flags);
  if (flags == NULL) {
    CFRelease(url);
    CFRelease(open_options);
    CFRelease(mount_options);
    return ENOMEM;
  }
  CFDictionarySetValue(mount_options, kNetFSMountFlagsKey, flags);

  AsyncRequestID request_id = NULL;
  int status = NetFSMountURLAsync(
      url, NULL, NULL, NULL, open_options, mount_options, &request_id,
      dispatch_get_global_queue(QOS_CLASS_UTILITY, 0),
      ^(int completion_status, AsyncRequestID ignored_request_id,
        CFArrayRef mountpoints) {
        (void)ignored_request_id;
        char *mountpoint = classick_copy_mountpoint(mountpoints);
        completion(context, completion_status, mountpoint);
        free(mountpoint);
      });

  CFRelease(flags);
  CFRelease(url);
  CFRelease(open_options);
  CFRelease(mount_options);
  return status;
}
