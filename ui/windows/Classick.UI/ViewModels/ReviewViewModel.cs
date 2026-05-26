using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Classick_UI.Ipc;

namespace Classick_UI.ViewModels;

/// <summary>
/// Backs the Review page: mirrors the wire-side <see cref="ReviewEvent"/>
/// (with optional <see cref="HeaderEvent"/> path context) and exposes the
/// three terminal actions a user can take (Apply / DryRun / Quit).
///
/// <para>
/// The VM does NOT talk to <c>CoreProcess</c> directly — that keeps it pure
/// and testable without spinning a subprocess. Instead it raises
/// <see cref="DecisionMade"/> with the typed <see cref="ReviewDecisionCommand"/>
/// envelope; the page's host (typically <c>MainPage</c> or <c>App</c>)
/// subscribes and forwards to <c>ICoreProcess.SendAsync</c>.
/// </para>
///
/// <para>
/// Uses CommunityToolkit.Mvvm partial-property syntax (matches
/// <c>MainPageViewModel</c>) so the source generator emits WinRT-compatible
/// marshalling code (MVVMTK0045).
/// </para>
/// </summary>
public partial class ReviewViewModel : ObservableObject
{
    [ObservableProperty] public partial string Source { get; set; } = "";
    [ObservableProperty] public partial string Ipod { get; set; } = "";
    [ObservableProperty] public partial string Manifest { get; set; } = "";

    [ObservableProperty] public partial int Add { get; set; }
    [ObservableProperty] public partial int Modify { get; set; }
    [ObservableProperty] public partial int MetadataOnly { get; set; }
    [ObservableProperty] public partial int Remove { get; set; }
    [ObservableProperty] public partial int Unchanged { get; set; }

    [ObservableProperty] public partial bool NoDelete { get; set; }

    /// <summary>
    /// True while the VM is awaiting the user's decision. False both before
    /// the Review event arrives and after a button is clicked (prevents
    /// double-submission).
    /// </summary>
    [ObservableProperty] public partial bool CanDecide { get; set; }

    /// <summary>
    /// Effective Remove count after the user's no-delete toggle.
    /// </summary>
    public int EffectiveRemove => NoDelete ? 0 : Remove;

    /// <summary>
    /// Sum of all changes that will actually apply (matches Rust's
    /// total_planned calculation: add + modify + metadata_only + effective_remove).
    /// </summary>
    public int TotalToApply => Add + Modify + MetadataOnly + EffectiveRemove;

    partial void OnRemoveChanged(int value)
    {
        OnPropertyChanged(nameof(EffectiveRemove));
        OnPropertyChanged(nameof(TotalToApply));
    }

    partial void OnAddChanged(int value) => OnPropertyChanged(nameof(TotalToApply));
    partial void OnModifyChanged(int value) => OnPropertyChanged(nameof(TotalToApply));
    partial void OnMetadataOnlyChanged(int value) => OnPropertyChanged(nameof(TotalToApply));

    partial void OnNoDeleteChanged(bool value)
    {
        OnPropertyChanged(nameof(EffectiveRemove));
        OnPropertyChanged(nameof(TotalToApply));
    }

    /// <summary>
    /// Apply the snapshot data from a Review IPC event. Call on the UI thread.
    /// </summary>
    public void LoadFromEvent(ReviewEvent evt, HeaderEvent? header = null)
    {
        if (header is not null)
        {
            Source = header.Source;
            Ipod = header.Ipod;
            Manifest = header.Manifest;
        }
        Add = evt.Summary.Add;
        Modify = evt.Summary.Modify;
        MetadataOnly = evt.Summary.MetadataOnly;
        Remove = evt.Summary.Remove;
        Unchanged = evt.Summary.Unchanged;
        NoDelete = evt.NoDelete;
        CanDecide = true;
    }

    /// <summary>
    /// Raised when the user makes a decision. The host (typically MainPage
    /// or App) subscribes and forwards to CoreProcess.SendAsync.
    /// </summary>
    public event System.Action<ReviewDecisionCommand>? DecisionMade;

    [RelayCommand(CanExecute = nameof(CanDecide))]
    private void Apply()
    {
        CanDecide = false;
        DecisionMade?.Invoke(new ReviewDecisionCommand(new ApplyDecision(NoDelete)));
    }

    [RelayCommand(CanExecute = nameof(CanDecide))]
    private void DryRun()
    {
        CanDecide = false;
        DecisionMade?.Invoke(new ReviewDecisionCommand(new DryRunDecision()));
    }

    [RelayCommand(CanExecute = nameof(CanDecide))]
    private void Quit()
    {
        CanDecide = false;
        DecisionMade?.Invoke(new ReviewDecisionCommand(new QuitDecision()));
    }

    partial void OnCanDecideChanged(bool value)
    {
        ApplyCommand.NotifyCanExecuteChanged();
        DryRunCommand.NotifyCanExecuteChanged();
        QuitCommand.NotifyCanExecuteChanged();
    }
}
