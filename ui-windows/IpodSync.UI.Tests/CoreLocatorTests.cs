using System;
using System.Collections.Generic;
using System.IO;
using IpodSync_UI.Core;
using Xunit;

public class CoreLocatorTests
{
    [Fact]
    public void Find_with_explicit_existing_path_returns_it()
    {
        var temp = Path.Combine(Path.GetTempPath(), $"ipod-sync-test-{Guid.NewGuid():N}.exe");
        File.WriteAllText(temp, "");
        try
        {
            var found = CoreLocator.Find(temp);
            Assert.Equal(Path.GetFullPath(temp), found);
        }
        finally { File.Delete(temp); }
    }

    [Fact]
    public void Find_with_explicit_nonexistent_path_falls_through_to_other_resolution()
    {
        // Pass a path that doesn't exist; without sibling/path hits, expect CoreNotFoundException.
        var nonexistent = Path.Combine(Path.GetTempPath(), $"definitely-not-here-{Guid.NewGuid():N}.exe");
        var ex = Assert.Throws<CoreNotFoundException>(() => CoreLocator.Find(nonexistent));
        Assert.Contains(nonexistent, ex.Message);
        Assert.Contains("Tried (in order):", ex.Message);
    }

    [Fact]
    public void CoreNotFoundException_message_includes_remediation_steps()
    {
        var ex = new CoreNotFoundException(new List<string> { "/some/path" });
        Assert.Contains("cargo build --release", ex.Message);
        Assert.Contains("To fix:", ex.Message);
    }

    [Fact]
    public void CoreNotFoundException_lists_locations_in_order()
    {
        var locations = new List<string> { "a", "b", "c" };
        var ex = new CoreNotFoundException(locations);
        Assert.Contains("1. a", ex.Message);
        Assert.Contains("2. b", ex.Message);
        Assert.Contains("3. c", ex.Message);
    }
}
