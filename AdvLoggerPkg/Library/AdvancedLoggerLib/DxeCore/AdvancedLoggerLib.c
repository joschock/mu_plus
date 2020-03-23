/** @file
  DXE_CORE implementation of Advanced Logger Library.

  Copyright (c) Microsoft Corporation. All rights reserved.<BR>
  SPDX-License-Identifier: BSD-2-Clause-Patent

**/

#include <Base.h>

#include <AdvancedLoggerInternal.h>

#include <Protocol/AdvancedLogger.h>

#include <Library/BaseLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/DebugLib.h>
#include <Library/HobLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PcdLib.h>
#include <Library/SerialPortLib.h>
#include <Library/SynchronizationLib.h>

#include "../AdvancedLoggerCommon.h"

//
// Protocol interface that connects the DXE library instances with the AdvancedLogger
//
STATIC ADVANCED_LOGGER_INFO    *mLoggerInfo = NULL;
STATIC VOID                    *mVariableWriteRegistration = NULL;

STATIC ADVANCED_LOGGER_PROTOCOL mLoggerProtocol = {
                                                   ADVANCED_LOGGER_PROTOCOL_SIGNATURE,
                                                   0,
                                                   AdvancedLoggerWrite,
                                                   0
};

/**
  Get the Logger Information block

 **/
ADVANCED_LOGGER_INFO *
EFIAPI
AdvancedLoggerGetLoggerInfo (
    VOID
) {
    EFI_HOB_GUID_TYPE          *GuidHob;
    ADVANCED_LOGGER_PTR        *LogPtr;

    if (mLoggerInfo == NULL) {
        //
        // Locate the Logger Information block.
        //
        GuidHob = GetFirstGuidHob (&gAdvancedLoggerHobGuid);
        if (GuidHob != NULL) {
            LogPtr = (ADVANCED_LOGGER_PTR * ) GET_GUID_HOB_DATA(GuidHob);
            mLoggerInfo = ALI_FROM_PA(LogPtr->LoggerInfo);
            mLoggerProtocol.Context = (VOID *) mLoggerInfo;
            if (!mLoggerInfo->SerialInitialized) {
                SerialPortInitialize();
                mLoggerInfo->SerialInitialized = TRUE;
            }
        }
    }

    return mLoggerInfo;
}


/**
    OnVariableWriteNotification

    Writes the log locator variable.

  **/
STATIC
VOID
EFIAPI
OnVariableWriteNotification (
    IN  EFI_EVENT   Event,
    IN  VOID        *Context
    )
{
    IN EFI_SYSTEM_TABLE  *SystemTable;

    SystemTable = (EFI_SYSTEM_TABLE *) Context;

    DEBUG((DEBUG_INFO, "OnVariableWriteNotification, writing locator variable\n"));

    SystemTable->RuntimeServices->SetVariable (
        ADVANCED_LOGGER_LOCATOR_NAME,
        &gAdvancedLoggerHobGuid,
        EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS,
        sizeof(mLoggerInfo),
        (VOID *) &mLoggerInfo );

    SystemTable->BootServices->CloseEvent (Event);

    return;
}

/**
    ProcessVariableWriteRegistration

    This function registers for Variable Write being available.

    @param       VOID

    @retval      EFI_SUCCESS     Variable Write protocol registration successful
    @retval      error code      Something went wrong.

 **/
EFI_STATUS
ProcessVariableWriteRegistration (
    IN EFI_SYSTEM_TABLE  *SystemTable
    )
{
    EFI_STATUS      Status;
    EFI_EVENT       VariableWriteEvent;

    //
    // Always register for file system notifications.  They may arrive at any time.
    //
    DEBUG((DEBUG_INFO, "Registering for VariableWrite notification\n"));
    Status = SystemTable->BootServices->CreateEvent (
                               EVT_NOTIFY_SIGNAL,
                               TPL_CALLBACK,
                               OnVariableWriteNotification,
                               SystemTable,
                              &VariableWriteEvent);

    if (EFI_ERROR(Status)) {
        DEBUG((DEBUG_ERROR, "%a: failed to create Variable Notification callback event (%r)\n", __FUNCTION__, Status));
        goto Cleanup;
    }

    Status = SystemTable->BootServices->RegisterProtocolNotify (
                                          &gEfiVariableWriteArchProtocolGuid,
                                           VariableWriteEvent,
                                          &mVariableWriteRegistration);

    if (EFI_ERROR(Status)) {
        DEBUG((DEBUG_ERROR, "%a: failed to register for file system notifications (%r)\n", __FUNCTION__, Status));
        SystemTable->BootServices->CloseEvent (VariableWriteEvent);
        goto Cleanup;
    }

    Status = EFI_SUCCESS;

Cleanup:
    return Status;
}


/**
  DxeCore Advanced Logger initialization.
 **/
EFI_STATUS
EFIAPI
DxeCoreAdvancedLoggerLibConstructor (
    IN EFI_HANDLE        ImageHandle,
    IN EFI_SYSTEM_TABLE  *SystemTable
  ) {
    ADVANCED_LOGGER_INFO *LoggerInfo;
    EFI_STATUS            Status;

    LoggerInfo = AdvancedLoggerGetLoggerInfo ();    // Sets mLoggerInfo if LoggerInfo found

    //
    // For an implementation of the AdvancedLogger with a PEI implementation, there will be a
    // Logger Information block published and available.
    //
    if (LoggerInfo == NULL) {
        LoggerInfo = (ADVANCED_LOGGER_INFO *) AllocateReservedPages(FixedPcdGet32 (PcdAdvancedLoggerPages));
        if (LoggerInfo != NULL) {
            LoggerInfo->Signature = ADVANCED_LOGGER_SIGNATURE;
            LoggerInfo->LogBuffer = PA_FROM_PTR(LoggerInfo + 1);
            LoggerInfo->LogBufferSize = EFI_PAGES_TO_SIZE (FixedPcdGet32 (PcdAdvancedLoggerPages)) - sizeof(ADVANCED_LOGGER_INFO);
            LoggerInfo->LogCurrent = LoggerInfo->LogBuffer;
            mLoggerInfo = LoggerInfo;
            mLoggerProtocol.Context = (VOID *) mLoggerInfo;
        } else {
            DEBUG((DEBUG_ERROR, "%a: Error allocating Advanced Logger Buffer\n", __FUNCTION__));
        }
    } else {
        mLoggerProtocol.Context = (VOID *) mLoggerInfo;
    }

    Status = SystemTable->BootServices->InstallProtocolInterface (&ImageHandle,
                                                                  &gAdvancedLoggerProtocolGuid,
                                                                   EFI_NATIVE_INTERFACE,
                                                                  &mLoggerProtocol);
    if (EFI_ERROR(Status)) {
        DEBUG((DEBUG_ERROR, "%a: Error installing protocol - %r\n", Status));
        // If the protocol doesn't install, don't fail.
    }

    DEBUG((DEBUG_INFO, "%a Initialized. mLoggerInfo = %p\n", __FUNCTION__, mLoggerInfo));

    if (FeaturePcdGet(PcdAdvancedLoggerLocator)) {
        ProcessVariableWriteRegistration (SystemTable);
    }

    return EFI_SUCCESS;
}
