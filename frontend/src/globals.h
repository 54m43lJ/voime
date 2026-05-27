#pragma once

#include <windows.h>
#include <unknwn.h>

// TSF headers
#include <msctf.h>
#include <textstor.h>

// GUIDs for our COM objects
// CLSID_VoimeTextService: {A1B2C3D4-E5F6-7890-ABCD-EF1234567890}
DEFINE_GUID(CLSID_VoimeTextService,
    0xa1b2c3d4, 0xe5f6, 0x7890, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90);

// GUID for our TSF profile
// {B2C3D4E5-F6A7-8901-BCDE-F12345678901}
DEFINE_GUID(GUID_VoimeProfile,
    0xb2c3d4e5, 0xf6a7, 0x8901, 0xbc, 0xde, 0xf1, 0x23, 0x45, 0x67, 0x89, 0x01);

// Display attribute GUID
// {C3D4E5F6-A7B8-9012-CDEF-123456789012}
DEFINE_GUID(GUID_VoimeDisplayAttribute,
    0xc3d4e5f6, 0xa7b8, 0x9012, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12);

// COM boilerplate
#define VOIME_SAFE_RELEASE(p) do { if(p) { (p)->Release(); (p) = nullptr; } } while(0)
