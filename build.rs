fn main() {
    #[cfg(target_os = "windows")]
    {
        let manifest = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="asInvoker" uiAccess="false" />
    </requestedPrivileges></security>
  </trustInfo>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
      version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df"
      language="*" />
  </dependentAssembly></dependency>
</assembly>
"#;
        winresource::WindowsResource::new()
            .set_manifest(manifest)
            .compile()
            .expect("failed to embed the Windows common-controls manifest");
    }
}
