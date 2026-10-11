/* Stand-in for AOSP's <log/log.h>, which the Fraunhofer FDK AAC sources
   include when built for Android (they log a security event in one place).
   Outside the Android platform tree there is no such header, so these are
   no-ops. Used by scripts/package-android.sh. */
#pragma once
#define android_errorWriteLog(tag, subTag) ((void)0)
#define android_errorWriteWithInfoLog(tag, subTag, uid, data, len) ((void)0)
#ifndef ALOGE
#define ALOGE(...) ((void)0)
#define ALOGW(...) ((void)0)
#define ALOGI(...) ((void)0)
#define ALOGD(...) ((void)0)
#define ALOGV(...) ((void)0)
#endif
