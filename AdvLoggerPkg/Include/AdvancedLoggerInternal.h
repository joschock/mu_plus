/** @file AdvancedLoggerInternal.h

    Advanced Logger internal data structures


    Copyright (C) Microsoft Corporation. All rights reserved.
    SPDX-License-Identifier: BSD-2-Clause-Patent

**/

#ifndef __ADVANCED_LOGGER_INTERNAL_H__
#define __ADVANCED_LOGGER_INTERNAL_H__

#define ADVANCED_LOGGER_SIGNATURE     SIGNATURE_32('A','L','O','G')

//
// These Pcds are used to carve out a PEI memory buffer from the temporary RAM.
//
//  PcdAdvancedLoggerBase -        NULL = UEFI starts with PEI, and SEC provides no memory log buffer
//                              Address = UEFI starts with SEC, and SEC provided LogInfoPtr is at this address
//  PcdAdvancedLoggerPreMemPages - Size = Pages to allocate from temporary RAM (SEC or PEI Pre-memory)
//

//
// Logger Info structure
//

#pragma pack (push, 1)

typedef volatile struct {
    UINT32                Signature;
    UINT32                LogBufferSize;
    EFI_PHYSICAL_ADDRESS  LogBuffer;              // Fixed pointer to start of log
    EFI_PHYSICAL_ADDRESS  LogCurrent;             // Where to store next log entry.
    UINT32                DiscardedSize;          // Number of bytes of messages missed
    BOOLEAN               SerialInitialized;      // Serial port initialized
    BOOLEAN               InPermanentRAM;         // Log in permanent RAM
    BOOLEAN               ExitBootServices;       // Exit Boot Services occurred
    BOOLEAN               PeiAllocated;           // Pei allocated "Temp Ram"
} ADVANCED_LOGGER_INFO;

typedef struct {
    UINT32                Signature;              // Signature
    UINT32                DebugLevel;             // Debug Level
    UINT64                TimeStamp;              // Time stamp
    UINT16                MessageLen;             // Number of bytes in Message
    CHAR8                 MessageText[];          // Message Text
} ADVANCED_LOGGER_MESSAGE_ENTRY;

#define MESSAGE_ENTRY_SIZE(LenOfMessage) (ALIGN_VALUE(sizeof(ADVANCED_LOGGER_MESSAGE_ENTRY) + LenOfMessage ,8))

#define NEXT_LOG_ENTRY(LogEntry) ((ADVANCED_LOGGER_MESSAGE_ENTRY *) ((UINTN) LogEntry + MESSAGE_ENTRY_SIZE(LogEntry->MessageLen)))

#define MESSAGE_ENTRY_SIGNATURE SIGNATURE_32('A','L','M','S')

#define MESSAGE_ENTRY_FROM_MSG(a)  BASE_CR (a, ADVANCED_LOGGER_MESSAGE_ENTRY, MessageText)

//
//  Insure the size of is a multiple of 8 bytes
//
STATIC_ASSERT (sizeof(ADVANCED_LOGGER_INFO) % 8 == 0, "Logger Info Misaligned" );

#pragma pack (pop)

//
// Access methods to convert between EFI_PHYSICAL_ADDRESS and UINT64 or CHAR8*
//
#define UINT64_FROM_PA(Address) ((UINT64) (UINTN) (Address))
#define ALI_FROM_PA(Address) ((ADVANCED_LOGGER_INFO *) (UINTN) (Address))
#define CHAR8_FROM_PA(Address) ((CHAR8 *) (UINTN) (Address))

#define PA_FROM_PTR(Address) ((EFI_PHYSICAL_ADDRESS) (UINTN) (Address))
#define PTR_FROM_PA(Address) ((VOID *) (UINTN) (Address))

//
// Log Buffer Base PCD points to this structure.  This is also the structure of the
// Advanced Logger HOB.
//
typedef struct {
    EFI_PHYSICAL_ADDRESS    LoggerInfo;
} ADVANCED_LOGGER_PTR;

#define ADVANCED_LOGGER_LOCATOR_NAME L"AdvLoggerLocator"

extern EFI_GUID gAdvancedLoggerHobGuid;

#endif  // __ADVANCED_LOGGER_INTERNAL_H__
